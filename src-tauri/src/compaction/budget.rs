use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

#[derive(Debug, thiserror::Error)]
pub enum BudgetError {
    #[error(
        "compaction_trigger ({trigger}) must be greater than target_after_compaction ({target})"
    )]
    TriggerBelowTarget { trigger: f32, target: f32 },
    #[error("percentages must be in (0.0, 1.0), got trigger={trigger}, target={target}")]
    InvalidPercentage { trigger: f32, target: f32 },
    #[error("reserved tokens ({reserved}) exceed max_tokens ({max})")]
    ReservedExceedsMax { reserved: usize, max: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBudget {
    pub max_tokens: usize,
    pub reserved_system: usize,
    pub reserved_response: usize,
    pub compaction_trigger: f32,
    pub target_after_compaction: f32,
}

impl ContextBudget {
    #[instrument]
    pub fn new(
        max_tokens: usize,
        reserved_system: usize,
        reserved_response: usize,
        compaction_trigger: f32,
        target_after_compaction: f32,
    ) -> Result<Self, BudgetError> {
        if !(0.0 < compaction_trigger && compaction_trigger < 1.0)
            || !(0.0 < target_after_compaction && target_after_compaction < 1.0)
        {
            return Err(BudgetError::InvalidPercentage {
                trigger: compaction_trigger,
                target: target_after_compaction,
            });
        }

        if compaction_trigger <= target_after_compaction {
            return Err(BudgetError::TriggerBelowTarget {
                trigger: compaction_trigger,
                target: target_after_compaction,
            });
        }

        let reserved = reserved_system + reserved_response;
        if reserved >= max_tokens {
            return Err(BudgetError::ReservedExceedsMax {
                reserved,
                max: max_tokens,
            });
        }

        debug!(
            max_tokens,
            reserved_system,
            reserved_response,
            compaction_trigger,
            target_after_compaction,
            "context budget created"
        );

        Ok(Self {
            max_tokens,
            reserved_system,
            reserved_response,
            compaction_trigger,
            target_after_compaction,
        })
    }

    pub fn available_tokens(&self) -> usize {
        self.max_tokens - self.reserved_system - self.reserved_response
    }

    pub fn target_token_count(&self) -> usize {
        (self.max_tokens as f32 * self.target_after_compaction) as usize
    }

    pub fn trigger_token_count(&self) -> usize {
        (self.max_tokens as f32 * self.compaction_trigger) as usize
    }

    pub fn needs_deep_compaction(&self, current_tokens: usize) -> bool {
        let needs = current_tokens >= self.trigger_token_count();
        debug!(
            current_tokens,
            trigger = self.trigger_token_count(),
            needs_compaction = needs,
            "deep compaction check"
        );
        needs
    }

    pub fn tokens_to_reclaim(&self, current_tokens: usize) -> usize {
        if self.needs_deep_compaction(current_tokens) {
            let reclaim = current_tokens.saturating_sub(self.target_token_count());
            debug!(
                current_tokens,
                target = self.target_token_count(),
                reclaim,
                "tokens to reclaim"
            );
            reclaim
        } else {
            0
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_snippet: Option<String>,
}

/// Estimates token count for a string using a simple heuristic.
/// Uses ~4 chars per token as a rough approximation.
/// This is intentionally simple; swap for tiktoken-rs or similar later.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    (text.len() + 3) / 4
}

/// Estimates total token count for a slice of messages.
/// Adds a per-message overhead of 4 tokens for role/framing.
pub fn estimate_message_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|m| estimate_tokens(&m.content) + 4)
        .sum()
}
