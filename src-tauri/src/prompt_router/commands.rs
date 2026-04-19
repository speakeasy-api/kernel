use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;
use tauri::{Emitter, State};
use tokio::sync::Mutex;
use tracing::{debug, error, info, instrument};
use uuid::Uuid;

use crate::agentkit_bridge;
use crate::anthropic::pricing::calculate_cost;
use crate::anthropic::types::{ContentBlock, Message, Role};
use crate::anthropic::LlmClient2;
use crate::config::load_config;
use crate::events::emit::emit;
use crate::events::EventData;
use crate::modes::builtin::builtin_modes;
use crate::prompt_router::classify::LlmClient;
use crate::prompt_router::dispatch::{
    dispatch, AgentHandoff, DispatchError, LoadedMode, ModeLoader, RouterEventSink,
};
use crate::prompt_router::model_registry::{ModelRegistry, FALLBACK_MODEL};
use crate::prompt_router::types::*;
use crate::prompt_router::user_override::ModeOverriddenEvent;

use std::collections::HashMap;

// ---- Per-session cancellation flags ----

pub struct CancellationFlags {
    inner: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl CancellationFlags {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Create a fresh (unset) flag for a session, returning a handle to it.
    /// Returns an error if a prompt is already running for this session.
    async fn create(&self, session_id: &str) -> Result<Arc<AtomicBool>, String> {
        let mut map = self.inner.lock().await;
        if map.contains_key(session_id) {
            return Err(format!(
                "a prompt is already running for session {session_id}"
            ));
        }
        let flag = Arc::new(AtomicBool::new(false));
        map.insert(session_id.to_string(), Arc::clone(&flag));
        Ok(flag)
    }

    /// Signal cancellation for a session.
    async fn cancel(&self, session_id: &str) {
        if let Some(flag) = self.inner.lock().await.get(session_id) {
            flag.store(true, Ordering::SeqCst);
        }
    }

    /// Remove the flag once the loop has finished.
    async fn remove(&self, session_id: &str) {
        self.inner.lock().await.remove(session_id);
    }
}

/// RAII guard that removes the cancellation flag when dropped.
struct CancellationGuard {
    session_id: String,
    flags: Arc<CancellationFlags>,
}

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        let session_id = self.session_id.clone();
        let flags = Arc::clone(&self.flags);
        // Spawn a task to do the async cleanup since Drop is synchronous
        tokio::spawn(async move {
            flags.remove(&session_id).await;
        });
    }
}

// ---- DB-backed conversation helpers ----

/// Loaded agent context with per-message metadata that the agentkit bridge
/// uses to stamp pinned/ordinal info onto transcript items.
pub struct AgentContext {
    pub messages: Vec<Message>,
    /// `ordinals[i] == None` for entries that have no DB row (e.g. items
    /// that came from a snapshot summary).
    pub ordinals: Vec<Option<i64>>,
    /// Map: message index -> existing context_snippet (if any). Presence
    /// in the map indicates the message is pinned.
    pub pinned_data: HashMap<usize, Option<String>>,
}

impl AgentContext {
    pub fn into_messages(self) -> Vec<Message> {
        self.messages
    }
}

/// Load the agent's working context: latest snapshot + messages after it.
/// Falls back to all messages if no snapshot exists. Pinned messages that
/// were compacted away are re-injected after the snapshot summary.
///
/// Returns the messages alongside per-message DB ordinals and pinned-data
/// so the agentkit bridge can stamp transcript items for compaction.
#[instrument(skip(pool))]
async fn load_agent_context(pool: &SqlitePool, session_id: &str) -> Result<AgentContext, String> {
    let mut messages: Vec<Message> = Vec::new();
    let mut ordinals: Vec<Option<i64>> = Vec::new();
    let mut pinned_data: HashMap<usize, Option<String>> = HashMap::new();

    if let Some(snapshot) = crate::db::queries::get_latest_snapshot(pool, session_id)
        .await
        .map_err(|e| e.to_string())?
    {
        // Snapshot summary entries — no DB rows.
        let summary: Vec<Message> = serde_json::from_str(&snapshot.summary_messages)
            .map_err(|e| format!("failed to deserialize snapshot: {e}"))?;
        for msg in summary {
            ordinals.push(None);
            messages.push(msg);
        }

        // Pinned messages from before the snapshot — re-injected verbatim
        // (with their context snippet attached for the model).
        let pinned = crate::db::queries::get_pinned_messages(pool, session_id)
            .await
            .map_err(|e| e.to_string())?;

        for row in &pinned {
            if row.ordinal <= snapshot.up_to_ordinal {
                let content: Vec<ContentBlock> = serde_json::from_str(&row.content)
                    .map_err(|e| format!("failed to deserialize pinned message: {e}"))?;
                let role = if row.role == "assistant" {
                    Role::Assistant
                } else {
                    Role::User
                };

                let content = if let Some(ref snippet) = row.context_snippet {
                    if !snippet.is_empty() {
                        let mut enriched = vec![ContentBlock::Text {
                            text: format!("[Pinned context: {snippet}]"),
                        }];
                        enriched.extend(content);
                        enriched
                    } else {
                        content
                    }
                } else {
                    content
                };

                let idx = messages.len();
                pinned_data.insert(idx, row.context_snippet.clone());
                ordinals.push(Some(row.ordinal));
                messages.push(Message { role, content });
            }
        }

        // Tail: messages after the snapshot.
        let tail = crate::db::queries::get_conversation_messages_since(
            pool,
            session_id,
            snapshot.up_to_ordinal,
        )
        .await
        .map_err(|e| e.to_string())?;

        for row in tail {
            let content: Vec<ContentBlock> = serde_json::from_str(&row.content)
                .map_err(|e| format!("failed to deserialize message content: {e}"))?;
            let role = if row.role == "assistant" {
                Role::Assistant
            } else {
                Role::User
            };
            let idx = messages.len();
            if row.pinned {
                pinned_data.insert(idx, row.context_snippet.clone());
            }
            ordinals.push(Some(row.ordinal));
            messages.push(Message { role, content });
        }
    } else {
        // No snapshot — load full history.
        let rows = crate::db::queries::get_conversation_messages(pool, session_id)
            .await
            .map_err(|e| e.to_string())?;
        for row in rows {
            let content: Vec<ContentBlock> = serde_json::from_str(&row.content)
                .map_err(|e| format!("failed to deserialize message content: {e}"))?;
            let role = if row.role == "assistant" {
                Role::Assistant
            } else {
                Role::User
            };
            let idx = messages.len();
            if row.pinned {
                pinned_data.insert(idx, row.context_snippet.clone());
            }
            ordinals.push(Some(row.ordinal));
            messages.push(Message { role, content });
        }
    }

    Ok(AgentContext {
        messages,
        ordinals,
        pinned_data,
    })
}

/// Append a Message to the conversation log and return its ordinal.
async fn append_message(
    pool: &SqlitePool,
    session_id: &str,
    message: &Message,
) -> Result<i64, String> {
    append_message_pinned(pool, session_id, message, false).await
}

/// Append a Message to the conversation log with a pin flag and return its ordinal.
async fn append_message_pinned(
    pool: &SqlitePool,
    session_id: &str,
    message: &Message,
    pinned: bool,
) -> Result<i64, String> {
    let role_str = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let content_json =
        serde_json::to_string(&message.content).map_err(|e| format!("serialize failed: {e}"))?;
    crate::db::queries::append_conversation_message(pool, session_id, role_str, &content_json, pinned)
        .await
        .map_err(|e| e.to_string())
}

// ---- Event sink that discards (events are persisted separately) ----

struct NoopEventSink;

impl RouterEventSink for NoopEventSink {
    fn emit_prompt_classified(&self, _session_id: &str, _output: &RouterOutput) {}
    fn emit_mode_overridden(&self, _session_id: &str, _event: &ModeOverriddenEvent) {}
}

// ---- Mode loader backed by builtins ----

struct BuiltinModeLoader;

impl ModeLoader for BuiltinModeLoader {
    fn load_mode(&self, mode_name: &str) -> Result<LoadedMode, DispatchError> {
        builtin_modes()
            .into_iter()
            .find(|m| m.name == mode_name)
            .map(|m| LoadedMode {
                name: m.name,
                system_prompt: m.system_prompt,
                default_model: m.default_model,
                allowed_tools: m.allowed_tools,
            })
            .ok_or_else(|| DispatchError::ModeNotFound(mode_name.to_string()))
    }
}

// ---- Payloads emitted to frontend ----

#[derive(Clone, Serialize)]
struct LlmModeResolved {
    mode: String,
    model: String,
    confidence: f32,
}

#[derive(Clone, Serialize)]
struct LlmUsage {
    session_id: String,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
}

#[derive(Clone, Serialize)]
struct FileRevertedEvent {
    tool_use_id: String,
    path: String,
    reason: String,
}

// ---- Resolve LLM client from config ----

fn resolve_client(project_path: &str) -> Result<LlmClient2, String> {
    let config = load_config(Path::new(project_path)).unwrap_or_default();

    // Try each configured provider in order
    for (name, _pc) in &config.models.providers {
        if let Ok(client) = LlmClient2::from_config(&config.models, name) {
            debug!(provider = %name, "resolved LLM client from config");
            return Ok(client);
        }
    }

    // No configured providers worked — fall back to env detection
    debug!("falling back to env-based LLM client");
    LlmClient2::from_env()
}

// ---- Model unsupported error detection ----

fn is_model_unsupported_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("model not found")
        || lower.contains("unsupported model")
        || lower.contains("model is not available")
        || lower.contains("no endpoints found for model")
        || lower.contains("invalid model")
}

// ---- Commands ----

/// Return the raw conversation context for a session (what the LLM actually sees).
#[tauri::command]
#[instrument(skip(pool))]
pub async fn get_conversation_context(
    session_id: String,
    pool: State<'_, SqlitePool>,
) -> Result<Vec<Message>, String> {
    debug!(session_id, "fetching conversation context");
    load_agent_context(&*pool, &session_id)
        .await
        .map(AgentContext::into_messages)
}

/// Return the full conversation history for the UI timeline.
#[tauri::command]
#[instrument(skip(pool))]
pub async fn get_conversation_history(
    session_id: String,
    pool: State<'_, SqlitePool>,
) -> Result<ConversationHistory, String> {
    debug!(session_id, "fetching conversation history");
    build_conversation_history(&*pool, &session_id).await
}

/// Look up the context window size for a model via the OpenRouter-backed
/// registry. The UI uses this on session reload to seed the context ring
/// before the first live usage event arrives.
#[tauri::command]
#[instrument(skip(registry))]
pub async fn model_context_window(
    model: String,
    registry: State<'_, Arc<ModelRegistry>>,
) -> Result<Option<u64>, String> {
    registry.ensure_warm().await;
    Ok(registry.context_length_for_model(&model))
}

#[derive(Clone, Serialize)]
pub struct ConversationHistory {
    pub entries: Vec<HistoryEntry>,
    pub last_mode: Option<ModeResolvedInfo>,
}

#[derive(Clone, Serialize)]
pub struct ModeResolvedInfo {
    pub mode: String,
    pub model: String,
    pub confidence: f32,
}

#[derive(Clone, Serialize)]
#[serde(tag = "type")]
pub enum HistoryEntry {
    #[serde(rename = "message")]
    Message {
        role: String,
        content: Vec<ContentBlock>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        pinned: bool,
    },
    #[serde(rename = "interrupted")]
    Interrupted,
    #[serde(rename = "compaction")]
    Compacted {
        before_messages: usize,
        after_messages: usize,
    },
}

async fn build_conversation_history(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<ConversationHistory, String> {
    // 1. Load all messages (ordered by ordinal)
    let rows = crate::db::queries::get_conversation_messages(pool, session_id)
        .await
        .map_err(|e| e.to_string())?;

    // 2. Load all events once
    let events = crate::db::queries::events_since(pool, session_id, "2000-01-01T00:00:00")
        .await
        .map_err(|e| e.to_string())?;

    // 3. Extract interrupted ordinals (sorted) so we can interleave them
    let mut interrupt_ordinals: Vec<i64> = events
        .iter()
        .filter(|e| e.kind == "Interrupted")
        .filter_map(|e| {
            let d: Value = serde_json::from_str(&e.data).ok()?;
            d.get("after_ordinal")?.as_i64()
        })
        .collect();
    interrupt_ordinals.sort();

    // 4. Extract compaction markers
    let compaction_markers: Vec<(usize, usize)> = events
        .iter()
        .filter(|e| e.kind == "ContextCompacted")
        .filter_map(|e| {
            let d: Value = serde_json::from_str(&e.data).ok()?;
            let before = d.get("before_messages")?.as_u64()? as usize;
            let after = d.get("after_messages")?.as_u64()? as usize;
            Some((before, after))
        })
        .collect();

    // 5. Load latest PromptClassified for last_mode
    let last_mode = events
        .iter()
        .rev()
        .find(|e| e.kind == "PromptClassified")
        .and_then(|e| {
            let d: Value = serde_json::from_str(&e.data).ok()?;
            Some(ModeResolvedInfo {
                mode: d.get("mode")?.as_str()?.to_string(),
                model: d.get("model")?.as_str()?.to_string(),
                confidence: d.get("confidence")?.as_f64()? as f32,
            })
        });

    // 6. Build entries, interleaving interrupted markers at correct positions
    let mut entries: Vec<HistoryEntry> = Vec::new();
    let mut interrupt_idx = 0;

    for row in &rows {
        // Insert any interrupted markers that belong before this message
        while interrupt_idx < interrupt_ordinals.len()
            && interrupt_ordinals[interrupt_idx] < row.ordinal
        {
            entries.push(HistoryEntry::Interrupted);
            interrupt_idx += 1;
        }

        let content: Vec<ContentBlock> = serde_json::from_str(&row.content).unwrap_or_default();
        entries.push(HistoryEntry::Message {
            role: row.role.clone(),
            content,
            pinned: row.pinned,
        });
    }

    // Append any remaining interrupted markers (after all messages)
    while interrupt_idx < interrupt_ordinals.len() {
        entries.push(HistoryEntry::Interrupted);
        interrupt_idx += 1;
    }

    // Append compaction markers at the end (session-level markers)
    for (before, after) in &compaction_markers {
        entries.push(HistoryEntry::Compacted {
            before_messages: *before,
            after_messages: *after,
        });
    }

    Ok(ConversationHistory { entries, last_mode })
}

#[tauri::command]
#[instrument(skip(app, pool))]
pub async fn revert_file(
    session_id: String,
    tool_use_id: String,
    path: String,
    before_content: Option<String>,
    after_content: String,
    reason: String,
    force: Option<bool>,
    app: tauri::AppHandle,
    pool: State<'_, SqlitePool>,
) -> Result<crate::tools::RevertResult, String> {
    info!(session_id, path = %path, tool_use_id, "revert_file called");

    let session = crate::db::queries::get_session(&*pool, &session_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("session not found: {session_id}"))?;

    let project = std::path::Path::new(&session.project_path);
    let result = crate::tools::revert_file_write(
        project,
        &path,
        before_content.as_deref(),
        &after_content,
        force.unwrap_or(false),
    )
    .await;

    if matches!(result, crate::tools::RevertResult::Success) {
        // Persist FileRevert event
        let revert_data = serde_json::json!({
            "tool_use_id": tool_use_id,
            "path": path,
            "reason": reason,
        })
        .to_string();
        let _ =
            crate::db::queries::insert_event(&*pool, &session_id, None, "FileRevert", &revert_data)
                .await;

        // Append revert info to conversation so LLM is aware
        let revert_msg = if reason.is_empty() {
            format!("I reverted your file write to {path}. The file has been restored to its previous state.")
        } else {
            format!("I reverted your file write to {path}. Reason: {reason}. The file has been restored to its previous state.")
        };
        let revert_message = crate::anthropic::types::Message {
            role: crate::anthropic::types::Role::User,
            content: vec![crate::anthropic::types::ContentBlock::Text { text: revert_msg }],
        };
        let _ = append_message(&*pool, &session_id, &revert_message).await;

        // Emit event for live frontend
        let _ = app.emit(
            "file-reverted",
            FileRevertedEvent {
                tool_use_id,
                path,
                reason,
            },
        );
    }

    Ok(result)
}

#[tauri::command]
#[instrument(skip(flags))]
pub async fn cancel_prompt(
    session_id: String,
    flags: State<'_, Arc<CancellationFlags>>,
) -> Result<(), String> {
    info!(session_id, "cancelling prompt");
    flags.cancel(&session_id).await;
    Ok(())
}

#[tauri::command]
#[instrument(
    skip(app, pool, registry, cancel_flags, prompt),
    fields(session_id, mode_override)
)]
pub async fn submit_prompt(
    session_id: String,
    prompt: String,
    mode_override: Option<String>,
    pinned: Option<bool>,
    app: tauri::AppHandle,
    pool: State<'_, SqlitePool>,
    registry: State<'_, Arc<ModelRegistry>>,
    cancel_flags: State<'_, Arc<CancellationFlags>>,
) -> Result<(), String> {
    let is_pinned = pinned.unwrap_or(false);
    info!(session_id, mode_override = ?mode_override, pinned = is_pinned, "submit_prompt called");

    // 0. Look up session to get project_path
    let session = crate::db::queries::get_session(&*pool, &session_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            error!(session_id, "session not found");
            format!("session not found: {session_id}")
        })?;

    let config = load_config(Path::new(&session.project_path)).unwrap_or_default();

    // 1. Persist PromptSubmitted event
    let event_data = serde_json::json!({
        "prompt": prompt,
        "mode": mode_override.as_deref().unwrap_or("auto"),
        "pinned": is_pinned,
    })
    .to_string();

    crate::db::queries::insert_event(&*pool, &session_id, None, "PromptSubmitted", &event_data)
        .await
        .map_err(|e| e.to_string())?;

    // 2. Resolve mode + model: reuse previous classification unless the user
    //    explicitly overrides the mode.  Only the first message in a session
    //    (or an explicit mode switch) triggers the LLM classifier.
    let previous = crate::db::queries::last_event_by_kind(&*pool, &session.id, "PromptClassified")
        .await
        .ok()
        .flatten()
        .and_then(|ev| serde_json::from_str::<serde_json::Value>(&ev.data).ok());

    let handoff: AgentHandoff = if mode_override.is_none() && previous.is_some() {
        // Reuse previous classification — no LLM call needed
        let prev = previous.unwrap();
        let mode_name = prev["mode"].as_str().unwrap_or("General").to_string();
        let model = prev["model"]
            .as_str()
            .unwrap_or(&config.models.default)
            .to_string();
        let confidence = prev["confidence"].as_f64().unwrap_or(1.0) as f32;

        let loaded = BuiltinModeLoader
            .load_mode(&mode_name)
            .map_err(|e| e.to_string())?;

        debug!(mode = %mode_name, model = %model, "reusing previous session classification");

        let _ = app.emit(
            "llm-mode-resolved",
            LlmModeResolved {
                mode: mode_name.clone(),
                model: model.clone(),
                confidence,
            },
        );

        AgentHandoff {
            mode_name,
            system_prompt: loaded.system_prompt,
            model,
            context: CompactedContext {
                messages_summary: String::new(),
                learnings: Vec::new(),
                preserved_facts: Vec::new(),
                token_count: 0,
            },
            allowed_tools: loaded.allowed_tools,
            prompt: prompt.clone(),
            confidence,
        }
    } else {
        // First message or explicit mode override — run the classifier
        let modes = builtin_modes();
        let available_modes: Vec<ModeInfo> = modes
            .iter()
            .map(|m| ModeInfo {
                name: m.name.clone(),
                description: m.description.clone(),
            })
            .collect();

        let router_input = RouterInput {
            source: PromptSource::User,
            prompt: prompt.clone(),
            available_modes,
            conversation_history: CompactedContext {
                messages_summary: String::new(),
                learnings: Vec::new(),
                preserved_facts: Vec::new(),
                token_count: 0,
            },
            project_context: ProjectContext::default(),
        };

        registry.ensure_warm().await;
        let all_models = registry.models_for_mode("general").await;
        let known_ids = registry.catalog_ids().await;

        let client = resolve_client(&session.project_path)?;
        let router_model = config.models.prompt_router.clone();
        let user_override = mode_override.clone();
        let config_default = config.models.default.clone();

        let mut h: AgentHandoff = {
            let input = router_input.clone();
            let rm = router_model;
            let uo = user_override;
            let models = all_models.clone();
            let cd = config_default;
            tokio::task::spawn_blocking(move || {
                dispatch(
                    &input,
                    uo.as_deref(),
                    &client as &dyn LlmClient,
                    &rm,
                    &BuiltinModeLoader,
                    &NoopEventSink,
                    &session_id,
                    &models,
                    &known_ids,
                    Some(cd.as_str()),
                )
            })
            .await
            .map_err(|e| format!("spawn_blocking failed: {e}"))?
            .map_err(|e| e.to_string())?
        };

        // Align the resolved model ID with the catalog's canonical form
        // so downstream usage (provider call, UI badge, context window
        // lookup) all reference the same string OpenRouter actually
        // accepts — e.g. `claude-sonnet-4-6` → `anthropic/claude-sonnet-4.6`.
        if let Some(canonical) = registry.resolve_catalog_id(&h.model) {
            if canonical != h.model {
                tracing::debug!(from = %h.model, to = %canonical, "canonicalized model id");
                h.model = canonical;
            }
        }

        // Persist classification for subsequent turns
        let classify_data = serde_json::json!({
            "mode": h.mode_name,
            "model": h.model,
            "confidence": h.confidence,
        })
        .to_string();
        let _ = crate::db::queries::insert_event(
            &*pool,
            &session.id,
            None,
            "PromptClassified",
            &classify_data,
        )
        .await;

        let _ = app.emit(
            "llm-mode-resolved",
            LlmModeResolved {
                mode: h.mode_name.clone(),
                model: h.model.clone(),
                confidence: h.confidence,
            },
        );

        h
    };

    // 7. Stream the response
    let model = handoff.model.clone();

    // Load conversation history from DB (with pin/ordinal metadata) and
    // append the new user message.
    let mut ctx = load_agent_context(&*pool, &session.id).await?;
    let user_msg = Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: prompt.clone(),
        }],
    };
    let user_ordinal = append_message_pinned(&*pool, &session.id, &user_msg, is_pinned).await?;
    if is_pinned {
        ctx.pinned_data.insert(ctx.messages.len(), None);
    }
    ctx.ordinals.push(Some(user_ordinal));
    ctx.messages.push(user_msg);

    let cancelled = cancel_flags.create(&session.id).await?;
    let _guard = CancellationGuard {
        session_id: session.id.clone(),
        flags: Arc::clone(&cancel_flags),
    };

    // Resolve context window from registry (real model metadata) or fall back
    let context_window = registry
        .context_length_for_model(&model)
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_CONTEXT_WINDOW);

    let mut working = ctx.messages.clone();
    let stream_result = agentkit_bridge::run_agent_loop(
        &handoff.system_prompt,
        &model,
        &handoff.allowed_tools,
        &session.project_path,
        &app,
        &mut working,
        &ctx.pinned_data,
        &ctx.ordinals,
        &session.id,
        &*pool,
        Arc::clone(&cancelled),
        &config,
        context_window,
    )
    .await;

    // 7b. Fallback retry on model-unsupported errors. The agentkit bridge
    // owns mid-loop and end-of-loop compaction + snapshot persistence — no
    // post-loop work needed here beyond the model-fallback path.
    let (full_text, accumulated_usage, model) = match stream_result {
        Ok((text, usage)) => (text, usage, model),
        Err(err) if is_model_unsupported_error(&err) => {
            let fallback_model = registry
                .resolve_catalog_id(FALLBACK_MODEL)
                .unwrap_or_else(|| FALLBACK_MODEL.to_string());

            tracing::warn!(
                model = %model,
                error = %err,
                fallback = %fallback_model,
                "model unsupported, retrying with fallback"
            );

            let _ = app.emit(
                "llm-mode-resolved",
                LlmModeResolved {
                    mode: handoff.mode_name.clone(),
                    model: fallback_model.clone(),
                    confidence: handoff.confidence,
                },
            );

            cancelled.store(false, Ordering::SeqCst);

            let fallback_context_window = registry
                .context_length_for_model(&fallback_model)
                .map(|n| n as usize)
                .unwrap_or(DEFAULT_CONTEXT_WINDOW);
            let mut fallback_working = ctx.messages.clone();
            let (text, usage) = agentkit_bridge::run_agent_loop(
                &handoff.system_prompt,
                &fallback_model,
                &handoff.allowed_tools,
                &session.project_path,
                &app,
                &mut fallback_working,
                &ctx.pinned_data,
                &ctx.ordinals,
                &session.id,
                &*pool,
                Arc::clone(&cancelled),
                &config,
                fallback_context_window,
            )
            .await?;
            (text, usage, fallback_model)
        }
        Err(err) => return Err(err),
    };

    // Persist an Interrupted event if the user cancelled
    if cancelled.load(Ordering::SeqCst) {
        let after_ordinal = crate::db::queries::get_max_ordinal(&*pool, &session.id)
            .await
            .ok()
            .flatten()
            .unwrap_or(0);
        let interrupt_data = serde_json::json!({
            "after_ordinal": after_ordinal,
        })
        .to_string();
        let _ = crate::db::queries::insert_event(
            &*pool,
            &session.id,
            None,
            "Interrupted",
            &interrupt_data,
        )
        .await;
    }

    let sid = session.id.clone();

    // 8. Persist usage events
    let cost_usd = calculate_cost(&model, &accumulated_usage);

    // Use a synthetic agent_id for this prompt-level usage tracking
    let agent_id = Uuid::new_v4();

    let _ = emit(
        &*pool,
        &sid,
        None,
        EventData::TokensUsed {
            agent_id,
            model: model.to_string(),
            input: accumulated_usage.input_tokens,
            output: accumulated_usage.output_tokens,
        },
    )
    .await;

    if cost_usd > 0.0 {
        let _ = emit(
            &*pool,
            &sid,
            None,
            EventData::CostIncurred {
                agent_id,
                model: model.to_string(),
                cost_usd,
            },
        )
        .await;
    }

    // 9. Emit usage to frontend
    let _ = app.emit(
        "llm-usage",
        LlmUsage {
            session_id: sid.to_string(),
            input_tokens: accumulated_usage.input_tokens,
            output_tokens: accumulated_usage.output_tokens,
            cost_usd,
        },
    );

    // 10. Persist AgentCompleted event
    let completion_data = serde_json::json!({
        "mode": handoff.mode_name,
        "model": model,
        "response": full_text,
    })
    .to_string();

    crate::db::queries::insert_event(&*pool, &sid, None, "AgentCompleted", &completion_data)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}


/// Conservative fallback when the model is not in the registry catalog.
const DEFAULT_CONTEXT_WINDOW: usize = 128_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_model_unsupported_error_detects_known_patterns() {
        assert!(is_model_unsupported_error("model not found: foo/bar"));
        assert!(is_model_unsupported_error("Unsupported Model: xyz"));
        assert!(is_model_unsupported_error("model is not available"));
        assert!(is_model_unsupported_error("No endpoints found for model"));
        assert!(is_model_unsupported_error("Invalid model ID specified"));
    }

    #[test]
    fn test_is_model_unsupported_error_ignores_unrelated() {
        assert!(!is_model_unsupported_error("rate limit exceeded"));
        assert!(!is_model_unsupported_error("connection timeout"));
        assert!(!is_model_unsupported_error(""));
    }
}
