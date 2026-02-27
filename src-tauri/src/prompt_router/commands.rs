use std::collections::BTreeMap;
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

use crate::anthropic::client::StreamRequest;
use crate::anthropic::pricing::calculate_cost;
use crate::anthropic::types::{ContentBlock, Message, Role, Usage};
use crate::anthropic::{LlmClient2, StreamChunk};
use crate::compaction;
use crate::config::{load_config, KernelConfig};
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
use crate::tools::{execute_tool, plans, tool_definitions};

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

/// Load the agent's working context: latest snapshot + messages after it.
/// Falls back to all messages if no snapshot exists.
/// Pinned messages that were compacted away are re-injected after the snapshot summary.
#[instrument(skip(pool))]
async fn load_agent_context(pool: &SqlitePool, session_id: &str) -> Result<Vec<Message>, String> {
    let mut rows = if let Some(snapshot) = crate::db::queries::get_latest_snapshot(pool, session_id)
        .await
        .map_err(|e| e.to_string())?
    {
        // Deserialize snapshot summary messages
        let mut messages: Vec<Message> = serde_json::from_str(&snapshot.summary_messages)
            .map_err(|e| format!("failed to deserialize snapshot: {e}"))?;

        // Load pinned messages from before the snapshot that were compacted away
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

                // Prepend context snippet if available
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

                messages.push(Message { role, content });
            }
        }

        // Load messages after the snapshot
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
            messages.push(Message { role, content });
        }

        messages
    } else {
        // No snapshot — load all messages
        let rows = crate::db::queries::get_conversation_messages(pool, session_id)
            .await
            .map_err(|e| e.to_string())?;

        let mut messages = Vec::with_capacity(rows.len());
        for row in rows {
            let content: Vec<ContentBlock> = serde_json::from_str(&row.content)
                .map_err(|e| format!("failed to deserialize message content: {e}"))?;
            let role = if row.role == "assistant" {
                Role::Assistant
            } else {
                Role::User
            };
            messages.push(Message { role, content });
        }
        messages
    };

    // Apply light compaction to loaded messages (truncate large tool results)
    light_compact(&mut rows);
    Ok(rows)
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
struct LlmChunk {
    text: String,
}

#[derive(Clone, Serialize)]
struct LlmDone {
    stop_reason: String,
    full_text: String,
}

#[derive(Clone, Serialize)]
struct LlmError {
    message: String,
}

#[derive(Clone, Serialize)]
struct LlmUsage {
    session_id: String,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
}

#[derive(Clone, Serialize)]
struct LlmToolCallEvent {
    id: String,
    name: String,
    input: Value,
}

#[derive(Clone, Serialize)]
struct LlmToolResultEvent {
    id: String,
    content: String,
    is_error: bool,
}

#[derive(Clone, Serialize)]
struct FileChangeEvent {
    tool_use_id: String,
    path: String,
    status: String,
    hunks: Vec<HunkPayload>,
    bytes_written: usize,
    before_content: Option<String>,
    after_content: String,
}

#[derive(Clone, Serialize)]
struct HunkPayload {
    header: String,
    lines: Vec<DiffLinePayload>,
}

#[derive(Clone, Serialize)]
struct DiffLinePayload {
    kind: String,
    content: String,
}

#[derive(Clone, Serialize)]
struct FileRevertedEvent {
    tool_use_id: String,
    path: String,
    reason: String,
}

struct InFlightToolCall {
    id: String,
    name: String,
    input_json: String,
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
    load_agent_context(&*pool, &session_id).await
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

        let h: AgentHandoff = {
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
    let stream_client = resolve_client(&session.project_path)?;
    let model = handoff.model.clone();

    // Load conversation history from DB and append user message
    let mut history = load_agent_context(&*pool, &session.id).await?;
    let user_msg = Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: prompt.clone(),
        }],
    };
    append_message_pinned(&*pool, &session.id, &user_msg, is_pinned).await?;
    history.push(user_msg);

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

    let mut working = history.clone();
    let stream_result = run_agent_loop(
        &stream_client,
        &handoff.system_prompt,
        &model,
        &handoff.allowed_tools,
        &session.project_path,
        &app,
        &mut working,
        &session.id,
        &*pool,
        &cancelled,
        &config,
        context_window,
    )
    .await;

    // Build pinned data map from DB for compaction awareness
    let pinned_data = build_pinned_data(&*pool, &session.id).await;

    // 7b. Fallback retry on model-unsupported errors
    let (full_text, accumulated_usage, model) = match stream_result {
        Ok((text, usage)) => {
            let pre_compact_len = working.len();
            let compact_result = maybe_compact_for_storage(
                working,
                &handoff.system_prompt,
                &session.project_path,
                &config,
                context_window,
                &pinned_data,
            )
            .await;
            // Write any pinned snippets back to DB
            write_pinned_snippets(&*pool, &session.id, &compact_result.pinned_snippets).await;
            // If compacted, save a context snapshot instead of modifying messages
            if compact_result.messages.len() < pre_compact_len {
                let compact_data = serde_json::json!({
                    "before_messages": pre_compact_len,
                    "after_messages": compact_result.messages.len(),
                })
                .to_string();
                let _ = crate::db::queries::insert_event(
                    &*pool,
                    &session.id,
                    None,
                    "ContextCompacted",
                    &compact_data,
                )
                .await;

                // Save the compacted messages as a snapshot
                if let Ok(Some(max_ord)) =
                    crate::db::queries::get_max_ordinal(&*pool, &session.id).await
                {
                    let summary_json = serde_json::to_string(&compact_result.messages).unwrap_or_default();
                    let _ = crate::db::queries::save_context_snapshot(
                        &*pool,
                        &session.id,
                        max_ord,
                        &summary_json,
                    )
                    .await;
                }
            }
            (text, usage, model)
        }
        Err(err) if is_model_unsupported_error(&err) => {
            tracing::warn!(
                model = %model,
                error = %err,
                fallback = FALLBACK_MODEL,
                "model unsupported, retrying with fallback"
            );

            // Emit updated mode-resolved with fallback model
            let _ = app.emit(
                "llm-mode-resolved",
                LlmModeResolved {
                    mode: handoff.mode_name.clone(),
                    model: FALLBACK_MODEL.to_string(),
                    confidence: handoff.confidence,
                },
            );

            // Reset the flag for the fallback attempt
            cancelled.store(false, Ordering::SeqCst);

            let fallback_client = resolve_client(&session.project_path)?;
            // Re-resolve context window for fallback model
            let fallback_context_window = registry
                .context_length_for_model(FALLBACK_MODEL)
                .map(|n| n as usize)
                .unwrap_or(DEFAULT_CONTEXT_WINDOW);
            let mut fallback_working = history.clone();
            let (text, usage) = run_agent_loop(
                &fallback_client,
                &handoff.system_prompt,
                FALLBACK_MODEL,
                &handoff.allowed_tools,
                &session.project_path,
                &app,
                &mut fallback_working,
                &session.id,
                &*pool,
                &cancelled,
                &config,
                fallback_context_window,
            )
            .await?;

            let pre_compact_len = fallback_working.len();
            let compact_result = maybe_compact_for_storage(
                fallback_working,
                &handoff.system_prompt,
                &session.project_path,
                &config,
                fallback_context_window,
                &pinned_data,
            )
            .await;
            write_pinned_snippets(&*pool, &session.id, &compact_result.pinned_snippets).await;
            if compact_result.messages.len() < pre_compact_len {
                let compact_data = serde_json::json!({
                    "before_messages": pre_compact_len,
                    "after_messages": compact_result.messages.len(),
                })
                .to_string();
                let _ = crate::db::queries::insert_event(
                    &*pool,
                    &session.id,
                    None,
                    "ContextCompacted",
                    &compact_data,
                )
                .await;

                if let Ok(Some(max_ord)) =
                    crate::db::queries::get_max_ordinal(&*pool, &session.id).await
                {
                    let summary_json = serde_json::to_string(&compact_result.messages).unwrap_or_default();
                    let _ = crate::db::queries::save_context_snapshot(
                        &*pool,
                        &session.id,
                        max_ord,
                        &summary_json,
                    )
                    .await;
                }
            }
            (text, usage, FALLBACK_MODEL.to_string())
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

/// Maximum agentic turns to prevent runaway loops.
const MAX_AGENT_TURNS: usize = 200;

/// Payload emitted each turn so the frontend can show real API token usage.
#[derive(Clone, Serialize)]
struct ContextUsage {
    session_id: String,
    input_tokens: u64,
    context_window: usize,
}

/// Run the agentic loop: stream LLM, execute tool calls, feed results back, repeat.
/// Appends assistant/tool messages to `messages` in place and persists to DB.
/// Returns `(full_text, accumulated_usage)` on success.
#[instrument(skip_all, fields(session_id, model, context_window))]
async fn run_agent_loop(
    client: &LlmClient2,
    system_prompt: &str,
    model: &str,
    allowed_tools: &[String],
    project_path: &str,
    app: &tauri::AppHandle,
    messages: &mut Vec<Message>,
    session_id: &str,
    pool: &SqlitePool,
    cancelled: &AtomicBool,
    config: &KernelConfig,
    context_window: usize,
) -> Result<(String, Usage), String> {
    info!(
        model,
        context_window,
        tools = allowed_tools.len(),
        "starting agent loop"
    );
    let project = Path::new(project_path);

    let mut full_text = String::new();
    let mut accumulated_usage = Usage::default();

    for turn in 0..MAX_AGENT_TURNS {
        // Check cancellation before starting a new turn
        if cancelled.load(Ordering::SeqCst) {
            let _ = app.emit(
                "llm-done",
                LlmDone {
                    stop_reason: "cancelled".into(),
                    full_text: full_text.clone(),
                },
            );
            return Ok((full_text, accumulated_usage));
        }

        // Build effective tool list per-turn (dynamic read_plan injection)
        let effective_tools =
            build_effective_tools(pool, session_id, allowed_tools).await;
        let tool_defs = tool_definitions(&effective_tools);

        let req = StreamRequest {
            system: system_prompt,
            messages: &*messages,
            model,
            max_tokens: 16384,
            tools: &tool_defs,
        };

        let mut rx = client.stream_message_full(&req).await?;

        let mut turn_text = String::new();
        let mut tool_calls: BTreeMap<u64, InFlightToolCall> = BTreeMap::new();
        let mut stop_reason = String::new();
        let mut turn_input_tokens: u64 = 0;

        while let Some(chunk) = rx.recv().await {
            // Check cancellation while consuming the stream
            if cancelled.load(Ordering::SeqCst) {
                // Save whatever text we've accumulated so far
                if !turn_text.is_empty() {
                    let partial_msg = Message {
                        role: Role::Assistant,
                        content: vec![ContentBlock::Text {
                            text: turn_text.clone(),
                        }],
                    };
                    let _ = append_message(pool, session_id, &partial_msg).await;
                    messages.push(partial_msg);
                }
                let _ = app.emit(
                    "llm-done",
                    LlmDone {
                        stop_reason: "cancelled".into(),
                        full_text: full_text.clone(),
                    },
                );
                return Ok((full_text, accumulated_usage));
            }

            match chunk {
                StreamChunk::Delta { text } => {
                    turn_text.push_str(&text);
                    full_text.push_str(&text);
                    let _ = app.emit("llm-chunk", LlmChunk { text });
                }
                StreamChunk::ToolUseStart { index, id, name } => {
                    tool_calls.insert(
                        index,
                        InFlightToolCall {
                            id,
                            name,
                            input_json: String::new(),
                        },
                    );
                }
                StreamChunk::ToolInputDelta {
                    index,
                    partial_json,
                } => {
                    if let Some(tc) = tool_calls.get_mut(&index) {
                        tc.input_json.push_str(&partial_json);
                    }
                }
                StreamChunk::ContentBlockStop { .. } => {}
                StreamChunk::Done { stop_reason: sr } => {
                    // Only take the first Done — message_stop can clobber
                    // the real stop_reason from message_delta.
                    if stop_reason.is_empty() {
                        stop_reason = sr;
                    }
                }
                StreamChunk::DoneWithUsage {
                    stop_reason: sr,
                    usage,
                } => {
                    if stop_reason.is_empty() {
                        stop_reason = sr;
                    }
                    // With prompt caching, input_tokens only counts non-cached
                    // tokens. The real context usage is the sum of all three.
                    turn_input_tokens = usage.input_tokens
                        + usage.cache_read_input_tokens
                        + usage.cache_creation_input_tokens;
                    accumulated_usage.merge(&usage);
                }
                StreamChunk::Error { message } => {
                    if is_model_unsupported_error(&message) {
                        return Err(message);
                    }
                    let _ = app.emit("llm-error", LlmError { message });
                    return Ok((full_text, accumulated_usage));
                }
                StreamChunk::MessageUsage { usage } => {
                    turn_input_tokens = usage.input_tokens
                        + usage.cache_read_input_tokens
                        + usage.cache_creation_input_tokens;
                    accumulated_usage.merge(&usage);
                }
            }
        }

        // Build assistant message content
        let mut assistant_content: Vec<ContentBlock> = Vec::new();
        if !turn_text.is_empty() {
            assistant_content.push(ContentBlock::Text { text: turn_text });
        }

        let mut finalized: Vec<(String, String, Value)> = Vec::new();
        for tc in tool_calls.into_values() {
            let input: Value = serde_json::from_str(&tc.input_json).unwrap_or(Value::Null);
            assistant_content.push(ContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.name.clone(),
                input: input.clone(),
            });
            finalized.push((tc.id, tc.name, input));
        }

        let assistant_msg = Message {
            role: Role::Assistant,
            content: assistant_content,
        };

        // Persist assistant message to DB
        let _ = append_message(pool, session_id, &assistant_msg).await;
        messages.push(assistant_msg);

        tracing::info!(
            turn,
            stop_reason = %stop_reason,
            tools = finalized.len(),
            "agent loop turn complete"
        );

        if stop_reason == "tool_use" && !finalized.is_empty() {
            // Execute each tool and collect results
            let mut tool_results: Vec<ContentBlock> = Vec::new();

            for (id, name, input) in &finalized {
                // Check cancellation before executing each tool
                if cancelled.load(Ordering::SeqCst) {
                    let _ = app.emit(
                        "llm-done",
                        LlmDone {
                            stop_reason: "cancelled".into(),
                            full_text: full_text.clone(),
                        },
                    );
                    return Ok((full_text, accumulated_usage));
                }

                // Persist tool call event (for audit)
                let tc_data =
                    serde_json::json!({"id": id, "name": name, "input": input}).to_string();
                let _ =
                    crate::db::queries::insert_event(pool, session_id, None, "ToolCall", &tc_data)
                        .await;

                let _ = app.emit(
                    "llm-tool-call",
                    LlmToolCallEvent {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    },
                );

                tracing::debug!(tool = %name, "executing tool");
                let tool_output = match name.as_str() {
                    "read_plan" => exec_read_plan(pool, session_id, project).await,
                    _ => execute_tool(name, input, project).await,
                };

                // Auto-attach plan after successful plan_create
                if name == "plan_create" {
                    if let Ok(ref output) = tool_output {
                        if let Some(filename) = plans::extract_filename_from_result(output) {
                            let _ = crate::db::queries::attach_plan(
                                pool, session_id, &filename,
                            )
                            .await;
                        }
                    }
                }

                let (content, is_error, file_change) = match &tool_output {
                    Ok(output) => {
                        tracing::debug!(tool = %name, bytes = output.content.len(), "tool ok");
                        (output.content.clone(), false, output.file_change.clone())
                    }
                    Err(err) => {
                        tracing::warn!(tool = %name, error = %err, "tool error");
                        (err.clone(), true, None)
                    }
                };

                // Persist tool result event (for audit)
                let tr_data =
                    serde_json::json!({"id": id, "content": content, "is_error": is_error})
                        .to_string();
                let _ = crate::db::queries::insert_event(
                    pool,
                    session_id,
                    None,
                    "ToolResult",
                    &tr_data,
                )
                .await;

                let _ = app.emit(
                    "llm-tool-result",
                    LlmToolResultEvent {
                        id: id.clone(),
                        content: content.clone(),
                        is_error,
                    },
                );

                // Emit and persist structured file-change data (for diff view + revert)
                if let Some(fc) = file_change {
                    let status_str = match &fc.status {
                        crate::tools::FileChangeStatus::Created => "created",
                        crate::tools::FileChangeStatus::Modified => "modified",
                    };
                    let hunks_payload: Vec<HunkPayload> = fc
                        .hunks
                        .iter()
                        .map(|h| HunkPayload {
                            header: h.header.clone(),
                            lines: h
                                .lines
                                .iter()
                                .map(|l| DiffLinePayload {
                                    kind: match l.kind {
                                        crate::git::diff::LineKind::Context => "context",
                                        crate::git::diff::LineKind::Add => "add",
                                        crate::git::diff::LineKind::Remove => "remove",
                                    }
                                    .into(),
                                    content: l.content.clone(),
                                })
                                .collect(),
                        })
                        .collect();

                    let fc_event = FileChangeEvent {
                        tool_use_id: id.clone(),
                        path: fc.path.clone(),
                        status: status_str.into(),
                        hunks: hunks_payload,
                        bytes_written: fc.bytes_written,
                        before_content: fc.before_content.clone(),
                        after_content: fc.after_content.clone(),
                    };

                    // Persist as DB event
                    let fc_data = serde_json::to_string(&fc_event).unwrap_or_default();
                    let _ = crate::db::queries::insert_event(
                        pool,
                        session_id,
                        None,
                        "FileChange",
                        &fc_data,
                    )
                    .await;

                    let _ = app.emit("file-change", fc_event);
                }

                tool_results.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content,
                    is_error,
                });
            }

            let tool_results_msg = Message {
                role: Role::User,
                content: tool_results,
            };

            // Persist tool results message to DB
            let _ = append_message(pool, session_id, &tool_results_msg).await;
            messages.push(tool_results_msg);

            // Light compaction: truncate large tool outputs from previous turns
            // (operates on working copy only — DB stores full content)
            light_compact(messages);

            // Emit real API token usage so the frontend can show accurate context ring
            let _ = app.emit(
                "context-usage",
                ContextUsage {
                    session_id: session_id.to_string(),
                    input_tokens: turn_input_tokens,
                    context_window,
                },
            );

            // Mid-loop deep compaction when real input_tokens exceed the trigger
            let trigger =
                (context_window as f64 * config.compaction.deep_trigger_pct as f64 / 100.0) as u64;
            if turn_input_tokens >= trigger {
                tracing::info!(
                    turn_input_tokens,
                    trigger,
                    context_window,
                    "mid-loop compaction triggered"
                );
                let mid_pinned_data = build_pinned_data(pool, session_id).await;
                let owned = std::mem::take(messages);
                let compact_result = maybe_compact_for_storage(
                    owned,
                    system_prompt,
                    project_path,
                    config,
                    context_window,
                    &mid_pinned_data,
                )
                .await;
                write_pinned_snippets(pool, session_id, &compact_result.pinned_snippets).await;
                *messages = compact_result.messages;
            }
            // Continue loop for next turn
        } else {
            // end_turn or other — done
            let _ = app.emit(
                "context-usage",
                ContextUsage {
                    session_id: session_id.to_string(),
                    input_tokens: turn_input_tokens,
                    context_window,
                },
            );
            let _ = app.emit(
                "llm-done",
                LlmDone {
                    stop_reason,
                    full_text: full_text.clone(),
                },
            );
            return Ok((full_text, accumulated_usage));
        }
    }

    // Reached max turns
    let _ = app.emit(
        "llm-done",
        LlmDone {
            stop_reason: "max_turns".into(),
            full_text: full_text.clone(),
        },
    );
    Ok((full_text, accumulated_usage))
}

// ---- Dynamic tool injection ----

/// Build the effective tool list for a turn, injecting `read_plan` when a plan is attached.
async fn build_effective_tools(
    pool: &SqlitePool,
    session_id: &str,
    allowed_tools: &[String],
) -> Vec<String> {
    let mut tools: Vec<String> = allowed_tools.to_vec();
    if let Ok(Some(_)) = crate::db::queries::get_attached_plan(pool, session_id).await {
        if !tools.iter().any(|t| t == "read_plan") {
            tools.push("read_plan".into());
        }
    }
    tools
}

/// Execute `read_plan`: reads the attached plan file, auto-detaches if deleted.
async fn exec_read_plan(
    pool: &SqlitePool,
    session_id: &str,
    project: &Path,
) -> Result<crate::tools::ToolOutput, String> {
    let filename = match crate::db::queries::get_attached_plan(pool, session_id).await {
        Ok(Some(f)) => f,
        Ok(None) => return Err("No plan is attached to this session.".into()),
        Err(e) => return Err(format!("DB error: {e}")),
    };

    let path = project.join(".kernel/plans").join(&filename);
    match tokio::fs::read_to_string(&path).await {
        Ok(content) => Ok(crate::tools::ToolOutput::text(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Auto-detach
            let _ = crate::db::queries::detach_plan(pool, session_id).await;
            Err(format!(
                "Attached plan '{filename}' no longer exists. It may have been removed externally."
            ))
        }
        Err(e) => Err(format!("Error reading plan '{filename}': {e}")),
    }
}

// ---- Light compaction (between agent turns) ----

/// Truncate large tool outputs in place to prevent context bloat between agent turns.
fn light_compact(messages: &mut [Message]) {
    for msg in messages.iter_mut() {
        for block in msg.content.iter_mut() {
            if let ContentBlock::ToolResult { content, .. } = block {
                let char_count = content.chars().count();
                if char_count > 2000 {
                    let head: String = content.chars().take(500).collect();
                    let tail: String = content.chars().skip(char_count - 200).collect();
                    *content =
                        format!("{head}\n... [truncated, {char_count} chars total] ...\n{tail}");
                }
            }
        }
    }
}

// ---- Deep compaction (on save to DB as snapshot) ----

struct CompactionClientAdapter {
    inner: LlmClient2,
    model: String,
}

#[async_trait::async_trait]
impl compaction::LlmClient for CompactionClientAdapter {
    async fn complete(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String, compaction::CompactionError> {
        self.inner
            .complete_system_async(system_prompt, user_prompt, &self.model, 4096)
            .await
            .map_err(compaction::CompactionError::LlmFailed)
    }
}

/// Convert anthropic messages to the compaction module's simple message format.
/// Accepts optional pinned data: maps message index -> (pinned, context_snippet).
fn to_compaction_messages(
    messages: &[Message],
    pinned_data: &HashMap<usize, Option<String>>,
) -> Vec<compaction::Message> {
    messages
        .iter()
        .enumerate()
        .map(|(i, msg)| {
            let has_tool_results = msg
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

            let role = if has_tool_results {
                "tool"
            } else {
                match msg.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                }
            }
            .to_string();

            let content: String = msg
                .content
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => text.clone(),
                    ContentBlock::ToolUse { id, name, input } => {
                        format!(
                            "[tool_use:{id}] {name}({})",
                            serde_json::to_string(input).unwrap_or_default()
                        )
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let status = if *is_error { "error" } else { "ok" };
                        format!("[tool_result:{tool_use_id}:{status}]\n{content}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");

            let (pinned, context_snippet) = if let Some(snippet) = pinned_data.get(&i) {
                (true, snippet.clone())
            } else {
                (false, None)
            };

            compaction::Message {
                role,
                content,
                pinned,
                context_snippet,
            }
        })
        .collect()
}

/// Convert compaction messages back to anthropic format (all as plain text).
fn from_compaction_messages(messages: &[compaction::Message]) -> Vec<Message> {
    let mut result: Vec<Message> = Vec::new();

    for msg in messages {
        let role = if msg.role == "assistant" {
            Role::Assistant
        } else {
            Role::User
        };

        // Merge consecutive same-role messages (API requires alternation)
        if let Some(last) = result.last_mut() {
            if last.role == role {
                last.content.push(ContentBlock::Text {
                    text: msg.content.clone(),
                });
                continue;
            }
        }

        result.push(Message {
            role,
            content: vec![ContentBlock::Text {
                text: msg.content.clone(),
            }],
        });
    }

    result
}

/// Conservative fallback when the model is not in the registry catalog.
const DEFAULT_CONTEXT_WINDOW: usize = 128_000;
const RESERVED_SYSTEM: usize = 2_000;
const RESERVED_RESPONSE: usize = 4_096;

/// Build pinned data map from DB: maps message index to context_snippet state.
/// This is a best-effort mapping — we match by ordinal position in the loaded messages.
async fn build_pinned_data(pool: &SqlitePool, session_id: &str) -> HashMap<usize, Option<String>> {
    let mut data = HashMap::new();
    let all_msgs = match crate::db::queries::get_conversation_messages(pool, session_id).await {
        Ok(rows) => rows,
        Err(_) => return data,
    };
    for (i, row) in all_msgs.iter().enumerate() {
        if row.pinned {
            data.insert(i, row.context_snippet.clone());
        }
    }
    data
}

/// Write pinned snippets returned from compaction back to the DB.
async fn write_pinned_snippets(
    pool: &SqlitePool,
    session_id: &str,
    snippets: &[compaction::PinnedSnippet],
) {
    if snippets.is_empty() {
        return;
    }
    // Map position-based snippets to real ordinals by looking up unprocessed pinned messages
    let unprocessed = match crate::db::queries::get_unprocessed_pinned(pool, session_id).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load unprocessed pinned messages for snippet write");
            return;
        }
    };
    for snippet in snippets {
        let ordinal = if (snippet.ordinal as usize) < unprocessed.len() {
            unprocessed[snippet.ordinal as usize].ordinal
        } else {
            tracing::warn!(
                position = snippet.ordinal,
                max = unprocessed.len(),
                "snippet position out of range, skipping"
            );
            continue;
        };
        let _ = crate::db::queries::update_context_snippet(
            pool,
            session_id,
            ordinal,
            &snippet.snippet,
        )
        .await;
    }
}

/// Compaction result returned to the caller so pinned snippets can be persisted.
struct CompactionResult {
    messages: Vec<Message>,
    pinned_snippets: Vec<compaction::PinnedSnippet>,
}

/// Run the compaction pipeline on messages before storing them.
/// Only triggers deep compaction when token count exceeds the budget trigger.
/// Falls back to returning originals if compaction fails.
#[instrument(skip_all, fields(message_count = messages.len(), context_window))]
async fn maybe_compact_for_storage(
    messages: Vec<Message>,
    system_prompt: &str,
    project_path: &str,
    config: &KernelConfig,
    context_window: usize,
    pinned_data: &HashMap<usize, Option<String>>,
) -> CompactionResult {
    // Estimate current token count
    let compact_msgs = to_compaction_messages(&messages, pinned_data);
    let token_estimate = compaction::estimate_message_tokens(&compact_msgs)
        + compaction::estimate_tokens(system_prompt);

    // Only trigger deep compaction when over budget
    let trigger = (context_window as f32 * config.compaction.deep_trigger_pct / 100.0) as usize;

    if token_estimate < trigger {
        return CompactionResult {
            messages,
            pinned_snippets: Vec::new(),
        };
    }

    tracing::info!(
        token_estimate,
        trigger,
        context_window,
        "context exceeds trigger, running deep compaction"
    );

    let client = match resolve_client(project_path) {
        Ok(c) => c,
        Err(_) => {
            return CompactionResult {
                messages,
                pinned_snippets: Vec::new(),
            }
        }
    };

    let adapter = CompactionClientAdapter {
        inner: client,
        model: config.models.compactor.clone(),
    };

    let pipeline = match compaction::CompactionPipeline::from_config(
        context_window,
        RESERVED_SYSTEM,
        RESERVED_RESPONSE,
        config.compaction.light_every_turn,
        config.compaction.deep_trigger_pct / 100.0,
        config.compaction.deep_target_pct / 100.0,
        adapter,
    ) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "failed to create compaction pipeline");
            return CompactionResult {
                messages,
                pinned_snippets: Vec::new(),
            };
        }
    };

    match pipeline.compact(system_prompt, &compact_msgs).await {
        Ok(ctx) => {
            let result = from_compaction_messages(&ctx.messages);
            tracing::info!(
                before = messages.len(),
                after = result.len(),
                tokens = ctx.token_count,
                pinned_snippets = ctx.pinned_snippets.len(),
                "deep compaction complete"
            );
            if result.is_empty() {
                CompactionResult {
                    messages,
                    pinned_snippets: ctx.pinned_snippets,
                }
            } else {
                CompactionResult {
                    messages: result,
                    pinned_snippets: ctx.pinned_snippets,
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "deep compaction failed");
            CompactionResult {
                messages,
                pinned_snippets: Vec::new(),
            }
        }
    }
}

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
