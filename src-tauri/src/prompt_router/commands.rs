use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;
use tauri::{Emitter, State};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::anthropic::client::StreamRequest;
use crate::anthropic::pricing::calculate_cost;
use crate::anthropic::types::{ContentBlock, Message, Role, Usage};
use crate::anthropic::{LlmClient2, StreamChunk};
use crate::tools::{execute_tool, tool_definitions};
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

// ---- Per-session conversation history ----

pub struct ConversationStore {
    inner: Mutex<HashMap<String, Vec<Message>>>,
}

impl ConversationStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    async fn get(&self, session_id: &str) -> Vec<Message> {
        self.inner
            .lock()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    async fn set(&self, session_id: &str, messages: Vec<Message>) {
        self.inner
            .lock()
            .await
            .insert(session_id.to_string(), messages);
    }
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
            return Ok(client);
        }
    }

    // No configured providers worked — fall back to env detection
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
pub async fn get_conversation_context(
    session_id: String,
    conversations: State<'_, Arc<ConversationStore>>,
) -> Result<Vec<Message>, String> {
    Ok(conversations.get(&session_id).await)
}

#[tauri::command]
pub async fn submit_prompt(
    session_id: String,
    prompt: String,
    mode_override: Option<String>,
    app: tauri::AppHandle,
    pool: State<'_, SqlitePool>,
    registry: State<'_, Arc<ModelRegistry>>,
    conversations: State<'_, Arc<ConversationStore>>,
) -> Result<(), String> {
    // 0. Look up session to get project_path
    let session = crate::db::queries::get_session(&*pool, &session_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("session not found: {session_id}"))?;

    let config = load_config(Path::new(&session.project_path)).unwrap_or_default();

    // 1. Persist PromptSubmitted event
    let event_data = serde_json::json!({
        "prompt": prompt,
        "mode": mode_override.as_deref().unwrap_or("auto"),
    })
    .to_string();

    crate::db::queries::insert_event(&*pool, &session_id, None, "PromptSubmitted", &event_data)
        .await
        .map_err(|e| e.to_string())?;

    // 2. Build RouterInput
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

    // 3. Gather available models from registry
    let all_models = registry.models_for_mode("general");

    // 4. Create LLM client from config → env fallback
    let client = resolve_client(&session.project_path)?;
    let router_model = config.models.prompt_router.clone();

    // 5. Classify mode (in spawn_blocking since dispatch is sync + calls LlmClient::complete)
    let user_override = mode_override.clone();
    let handoff: AgentHandoff = {
        let input = router_input.clone();
        let rm = router_model;
        let uo = user_override;
        let models = all_models.clone();
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
            )
        })
        .await
        .map_err(|e| format!("spawn_blocking failed: {e}"))?
        .map_err(|e| e.to_string())?
    };

    // 6. Persist classification + emit mode-resolved to frontend
    let classify_data = serde_json::json!({
        "mode": handoff.mode_name,
        "model": handoff.model,
        "confidence": handoff.confidence,
    })
    .to_string();
    let _ = crate::db::queries::insert_event(&*pool, &session.id, None, "PromptClassified", &classify_data).await;

    let _ = app.emit(
        "llm-mode-resolved",
        LlmModeResolved {
            mode: handoff.mode_name.clone(),
            model: handoff.model.clone(),
            confidence: handoff.confidence,
        },
    );

    // 7. Stream the response — use config default model if router left it empty
    let stream_client = resolve_client(&session.project_path)?;
    let model = if handoff.model.is_empty() {
        config.models.default.clone()
    } else {
        handoff.model.clone()
    };

    // Load conversation history and append user message
    let mut history = conversations.get(&session.id).await;
    history.push(Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: prompt.clone(),
        }],
    });

    // Save the user message immediately so Raw Context is never empty during streaming
    conversations.set(&session.id, history.clone()).await;

    let mut working = history.clone();
    let stream_result = run_agent_loop(
        &stream_client,
        &handoff.system_prompt,
        &model,
        &handoff.allowed_tools,
        &session.project_path,
        &app,
        &mut working,
        &conversations,
        &session.id,
        &*pool,
    )
    .await;

    // 7b. Fallback retry on model-unsupported errors
    let (full_text, accumulated_usage, model) = match stream_result {
        Ok((text, usage)) => {
            let pre_compact_len = working.len();
            let compacted = maybe_compact_for_storage(
                working,
                &handoff.system_prompt,
                &session.project_path,
                &config,
            )
            .await;
            // Persist compaction event if context was actually compacted
            if compacted.len() < pre_compact_len {
                let compact_data = serde_json::json!({
                    "before_messages": pre_compact_len,
                    "after_messages": compacted.len(),
                })
                .to_string();
                let _ = crate::db::queries::insert_event(&*pool, &session.id, None, "ContextCompacted", &compact_data).await;
            }
            conversations.set(&session.id, compacted).await;
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

            let fallback_client = resolve_client(&session.project_path)?;
            let mut fallback_working = history.clone();
            let (text, usage) = run_agent_loop(
                &fallback_client,
                &handoff.system_prompt,
                FALLBACK_MODEL,
                &handoff.allowed_tools,
                &session.project_path,
                &app,
                &mut fallback_working,
                &conversations,
                &session.id,
                &*pool,
            )
            .await?;

            let pre_compact_len = fallback_working.len();
            let compacted = maybe_compact_for_storage(
                fallback_working,
                &handoff.system_prompt,
                &session.project_path,
                &config,
            )
            .await;
            if compacted.len() < pre_compact_len {
                let compact_data = serde_json::json!({
                    "before_messages": pre_compact_len,
                    "after_messages": compacted.len(),
                })
                .to_string();
                let _ = crate::db::queries::insert_event(&*pool, &session.id, None, "ContextCompacted", &compact_data).await;
            }
            conversations.set(&session.id, compacted).await;
            (text, usage, FALLBACK_MODEL.to_string())
        }
        Err(err) => return Err(err),
    };

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

/// Run the agentic loop: stream LLM, execute tool calls, feed results back, repeat.
/// Appends assistant/tool messages to `messages` in place.
/// Returns `(full_text, accumulated_usage)` on success.
async fn run_agent_loop(
    client: &LlmClient2,
    system_prompt: &str,
    model: &str,
    allowed_tools: &[String],
    project_path: &str,
    app: &tauri::AppHandle,
    messages: &mut Vec<Message>,
    conversations: &ConversationStore,
    session_id: &str,
    pool: &SqlitePool,
) -> Result<(String, Usage), String> {
    let tool_defs = tool_definitions(allowed_tools);
    let project = Path::new(project_path);

    let mut full_text = String::new();
    let mut accumulated_usage = Usage::default();

    for turn in 0..MAX_AGENT_TURNS {
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

        while let Some(chunk) = rx.recv().await {
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
                StreamChunk::ToolInputDelta { index, partial_json } => {
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
                StreamChunk::Error { message } => {
                    if is_model_unsupported_error(&message) {
                        return Err(message);
                    }
                    let _ = app.emit("llm-error", LlmError { message });
                    return Ok((full_text, accumulated_usage));
                }
                StreamChunk::MessageUsage { usage } => {
                    accumulated_usage.merge(&usage);
                }
            }
        }

        // Persist assistant text to DB for history replay (before move)
        if !turn_text.is_empty() {
            let data = serde_json::json!({"text": turn_text}).to_string();
            let _ = crate::db::queries::insert_event(pool, session_id, None, "AssistantText", &data).await;
        }

        // Build assistant message content
        let mut assistant_content: Vec<ContentBlock> = Vec::new();
        if !turn_text.is_empty() {
            assistant_content.push(ContentBlock::Text { text: turn_text });
        }

        let mut finalized: Vec<(String, String, Value)> = Vec::new();
        for tc in tool_calls.into_values() {
            let input: Value =
                serde_json::from_str(&tc.input_json).unwrap_or(Value::Null);
            assistant_content.push(ContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.name.clone(),
                input: input.clone(),
            });
            finalized.push((tc.id, tc.name, input));
        }

        messages.push(Message {
            role: Role::Assistant,
            content: assistant_content,
        });

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
                // Persist tool call to DB
                let tc_data = serde_json::json!({"id": id, "name": name, "input": input}).to_string();
                let _ = crate::db::queries::insert_event(pool, session_id, None, "ToolCall", &tc_data).await;

                let _ = app.emit(
                    "llm-tool-call",
                    LlmToolCallEvent {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    },
                );

                tracing::debug!(tool = %name, "executing tool");
                let result = execute_tool(name, input, project).await;
                let (content, is_error) = match &result {
                    Ok(output) => {
                        tracing::debug!(tool = %name, bytes = output.len(), "tool ok");
                        (output.clone(), false)
                    }
                    Err(err) => {
                        tracing::warn!(tool = %name, error = %err, "tool error");
                        (err.clone(), true)
                    }
                };

                // Persist tool result to DB
                let tr_data = serde_json::json!({"id": id, "content": content, "is_error": is_error}).to_string();
                let _ = crate::db::queries::insert_event(pool, session_id, None, "ToolResult", &tr_data).await;

                let _ = app.emit(
                    "llm-tool-result",
                    LlmToolResultEvent {
                        id: id.clone(),
                        content: content.clone(),
                        is_error,
                    },
                );

                tool_results.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content,
                    is_error,
                });
            }

            messages.push(Message {
                role: Role::User,
                content: tool_results,
            });

            // Light compaction: truncate large tool outputs from previous turns
            light_compact(messages);

            // Save incrementally so Raw Context view stays up-to-date during streaming
            conversations.set(session_id, messages.clone()).await;
            // Continue loop for next turn
        } else {
            // end_turn or other — done
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
                    *content = format!(
                        "{head}\n... [truncated, {char_count} chars total] ...\n{tail}"
                    );
                }
            }
        }
    }
}

// ---- Deep compaction (on save to ConversationStore) ----

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
fn to_compaction_messages(messages: &[Message]) -> Vec<compaction::Message> {
    messages
        .iter()
        .map(|msg| {
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

            compaction::Message { role, content }
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

/// Context window size for budget calculations (Claude's context window).
const CONTEXT_WINDOW: usize = 200_000;
const RESERVED_SYSTEM: usize = 2_000;
const RESERVED_RESPONSE: usize = 4_096;

/// Run the compaction pipeline on messages before storing them.
/// Only triggers deep compaction when token count exceeds the budget trigger.
/// Falls back to returning originals if compaction fails.
async fn maybe_compact_for_storage(
    messages: Vec<Message>,
    system_prompt: &str,
    project_path: &str,
    config: &KernelConfig,
) -> Vec<Message> {
    // Estimate current token count
    let compact_msgs = to_compaction_messages(&messages);
    let token_estimate = compaction::estimate_message_tokens(&compact_msgs)
        + compaction::estimate_tokens(system_prompt);

    // Only trigger deep compaction when over budget
    let trigger =
        (CONTEXT_WINDOW as f32 * config.compaction.deep_trigger_pct / 100.0) as usize;

    if token_estimate < trigger {
        return messages;
    }

    tracing::info!(
        token_estimate,
        trigger,
        "context exceeds trigger, running deep compaction"
    );

    let client = match resolve_client(project_path) {
        Ok(c) => c,
        Err(_) => return messages,
    };

    let adapter = CompactionClientAdapter {
        inner: client,
        model: config.models.compactor.clone(),
    };

    let pipeline = match compaction::CompactionPipeline::from_config(
        CONTEXT_WINDOW,
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
            return messages;
        }
    };

    match pipeline.compact(system_prompt, &compact_msgs).await {
        Ok(ctx) => {
            let result = from_compaction_messages(&ctx.messages);
            tracing::info!(
                before = messages.len(),
                after = result.len(),
                tokens = ctx.token_count,
                "deep compaction complete"
            );
            if result.is_empty() {
                messages
            } else {
                result
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "deep compaction failed");
            messages
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
