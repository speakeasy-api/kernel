use std::sync::{Arc, Mutex};

use agentkit::compaction::CompactionReason;
use agentkit::core::{Delta, FinishReason, Part, Usage};
use agentkit::loop_::{AgentEvent, LoopObserver};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tracing::debug;

use crate::anthropic::types::Usage as KernelUsage;

#[derive(Clone, Serialize)]
struct LlmChunk {
    text: String,
}

#[derive(Clone, Serialize)]
struct LlmToolCallEvent {
    id: String,
    name: String,
    input: serde_json::Value,
}

#[derive(Clone, Serialize)]
struct ContextUsage {
    session_id: String,
    input_tokens: u64,
    context_window: usize,
}

#[derive(Clone, Serialize)]
struct CompactionStartedPayload {
    session_id: String,
    reason: String,
}

#[derive(Clone, Serialize)]
struct CompactionFinishedPayload {
    session_id: String,
    replaced_items: usize,
    transcript_len: usize,
}

/// Shared state the observer accumulates and the caller reads after the loop finishes.
#[derive(Default)]
pub struct AgentRunState {
    pub full_text: String,
    pub usage: KernelUsage,
    pub finish_reason: String,
}

pub struct TauriEventObserver {
    pub app: AppHandle,
    pub session_id: String,
    pub context_window: usize,
    pub state: Arc<Mutex<AgentRunState>>,
}

impl LoopObserver for TauriEventObserver {
    fn handle_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::ContentDelta(Delta::AppendText { chunk, .. }) => {
                if let Ok(mut s) = self.state.lock() {
                    s.full_text.push_str(&chunk);
                }
                let _ = self.app.emit("llm-chunk", LlmChunk { text: chunk });
            }
            AgentEvent::ContentDelta(Delta::CommitPart {
                part: Part::Text(t),
            }) => {
                if let Ok(s) = self.state.lock() {
                    if s.full_text.is_empty() {
                        drop(s);
                        if let Ok(mut s2) = self.state.lock() {
                            s2.full_text.push_str(&t.text);
                        }
                    }
                }
            }
            AgentEvent::ContentDelta(_) => {}
            AgentEvent::ToolCallRequested(call) => {
                let _ = self.app.emit(
                    "llm-tool-call",
                    LlmToolCallEvent {
                        id: call.id.0.clone(),
                        name: call.name.clone(),
                        input: call.input.clone(),
                    },
                );
            }
            AgentEvent::UsageUpdated(usage) => {
                apply_usage(&self.state, &usage);
                let input = usage
                    .tokens
                    .as_ref()
                    .map(total_input_tokens)
                    .unwrap_or(0);
                let _ = self.app.emit(
                    "context-usage",
                    ContextUsage {
                        session_id: self.session_id.clone(),
                        input_tokens: input,
                        context_window: self.context_window,
                    },
                );
            }
            AgentEvent::TurnFinished(result) => {
                if let Some(u) = &result.usage {
                    apply_usage(&self.state, u);
                }
                if let Ok(mut s) = self.state.lock() {
                    s.finish_reason = finish_reason_str(&result.finish_reason).into();
                }
                debug!(
                    turn_id = %result.turn_id.0,
                    finish = ?result.finish_reason,
                    "agentkit turn finished",
                );
            }
            AgentEvent::CompactionStarted { reason, .. } => {
                let _ = self.app.emit(
                    "compaction-started",
                    CompactionStartedPayload {
                        session_id: self.session_id.clone(),
                        reason: compaction_reason_str(&reason),
                    },
                );
            }
            AgentEvent::CompactionFinished {
                replaced_items,
                transcript_len,
                ..
            } => {
                let _ = self.app.emit(
                    "compaction-finished",
                    CompactionFinishedPayload {
                        session_id: self.session_id.clone(),
                        replaced_items,
                        transcript_len,
                    },
                );
                // Reset the context-usage ring on the frontend — token count
                // for the next turn will arrive via UsageUpdated.
                let _ = self.app.emit(
                    "context-usage",
                    ContextUsage {
                        session_id: self.session_id.clone(),
                        input_tokens: 0,
                        context_window: self.context_window,
                    },
                );
            }
            AgentEvent::TurnStarted { .. }
            | AgentEvent::RunStarted { .. }
            | AgentEvent::InputAccepted { .. }
            | AgentEvent::ApprovalRequired(_)
            | AgentEvent::AuthRequired(_)
            | AgentEvent::ApprovalResolved { .. }
            | AgentEvent::AuthResolved { .. }
            | AgentEvent::Warning { .. }
            | AgentEvent::RunFailed { .. } => {}
        }
    }
}

fn apply_usage(state: &Arc<Mutex<AgentRunState>>, usage: &Usage) {
    let Some(tokens) = &usage.tokens else { return };
    if let Ok(mut s) = state.lock() {
        let other = KernelUsage {
            input_tokens: tokens.input_tokens,
            output_tokens: tokens.output_tokens,
            cache_creation_input_tokens: tokens.cache_write_input_tokens.unwrap_or(0),
            cache_read_input_tokens: tokens.cached_input_tokens.unwrap_or(0),
        };
        s.usage.merge(&other);
    }
}

fn total_input_tokens(tokens: &agentkit::core::TokenUsage) -> u64 {
    tokens.input_tokens
        + tokens.cached_input_tokens.unwrap_or(0)
        + tokens.cache_write_input_tokens.unwrap_or(0)
}

pub fn finish_reason_str(reason: &FinishReason) -> &'static str {
    match reason {
        FinishReason::Completed => "end_turn",
        FinishReason::ToolCall => "tool_use",
        FinishReason::MaxTokens => "max_tokens",
        FinishReason::Cancelled => "cancelled",
        FinishReason::Blocked => "blocked",
        FinishReason::Error => "error",
        FinishReason::Other(_) => "other",
    }
}

fn compaction_reason_str(reason: &CompactionReason) -> String {
    match reason {
        CompactionReason::TranscriptTooLong => "transcript_too_long".into(),
        CompactionReason::Manual => "manual".into(),
        CompactionReason::Custom(s) => s.clone(),
    }
}
