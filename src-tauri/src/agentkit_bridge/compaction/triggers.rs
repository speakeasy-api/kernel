use std::sync::Arc;

use agentkit::compaction::{CompactionReason, CompactionTrigger};
use agentkit::core::{Item, ItemKind, Part, SessionId, ToolOutput, TurnId};

use crate::agentkit_bridge::convert::{items_to_messages, messages_to_compaction};
use crate::compaction::estimate_message_tokens;

/// Fire compaction when any tool-result text exceeds the threshold.
///
/// Light-only path: the truncate strategy that follows will reduce the
/// offending tool result in-place without dropping any items.
pub struct LargeToolResultTrigger {
    pub max_chars: usize,
}

impl CompactionTrigger for LargeToolResultTrigger {
    fn should_compact(
        &self,
        _session_id: &SessionId,
        _turn_id: Option<&TurnId>,
        transcript: &[Item],
    ) -> Option<CompactionReason> {
        for item in transcript {
            if item.kind != ItemKind::Tool {
                continue;
            }
            for part in &item.parts {
                if let Part::ToolResult(tr) = part {
                    if let ToolOutput::Text(text) = &tr.output {
                        if text.chars().count() > self.max_chars {
                            return Some(CompactionReason::Custom(
                                "tool result exceeds size threshold".into(),
                            ));
                        }
                    }
                }
            }
        }
        None
    }
}

/// Fire compaction when the estimated transcript token count crosses
/// `context_window * trigger_pct / 100`. Estimation skips pinned items so
/// that pinned-only growth doesn't oscillate the trigger.
///
/// Token estimation here mirrors the kernel `compaction::estimate_*`
/// helpers (~4 chars per token) used elsewhere for budget math, so the
/// trigger value matches user-visible "context used" in the UI.
pub struct TokenBudgetTrigger {
    pub context_window: usize,
    /// Same percentage as `KernelConfig::compaction::deep_trigger_pct`
    /// (range 0..100, e.g. 90.0 means trigger at 90% of context window).
    pub trigger_pct: f32,
    /// Tokens of the system prompt + reserved response budget — subtracted
    /// from the threshold so the trigger fires before the loop blows past
    /// the model's actual context limit.
    pub reserved_tokens: usize,
}

impl CompactionTrigger for TokenBudgetTrigger {
    fn should_compact(
        &self,
        _session_id: &SessionId,
        _turn_id: Option<&TurnId>,
        transcript: &[Item],
    ) -> Option<CompactionReason> {
        let usable = self.context_window.saturating_sub(self.reserved_tokens);
        let threshold =
            ((usable as f32) * (self.trigger_pct.clamp(0.0, 100.0) / 100.0)) as usize;
        if threshold == 0 {
            return None;
        }

        let messages = items_to_messages(transcript);
        let compact_msgs = messages_to_compaction(&messages);
        let tokens = estimate_message_tokens(&compact_msgs);
        if tokens >= threshold {
            Some(CompactionReason::Custom(format!(
                "transcript tokens {tokens} >= threshold {threshold}"
            )))
        } else {
            None
        }
    }
}

/// Composite trigger: fires if any inner trigger fires.
#[derive(Default)]
pub struct AnyOfTrigger {
    triggers: Vec<Arc<dyn CompactionTrigger>>,
}

impl AnyOfTrigger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, trigger: impl CompactionTrigger + 'static) -> Self {
        self.triggers.push(Arc::new(trigger));
        self
    }
}

impl CompactionTrigger for AnyOfTrigger {
    fn should_compact(
        &self,
        session_id: &SessionId,
        turn_id: Option<&TurnId>,
        transcript: &[Item],
    ) -> Option<CompactionReason> {
        for trigger in &self.triggers {
            if let Some(reason) = trigger.should_compact(session_id, turn_id, transcript) {
                return Some(reason);
            }
        }
        None
    }
}

