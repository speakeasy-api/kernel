use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::budget::{
    estimate_message_tokens, estimate_tokens, BudgetError, ContextBudget, Message,
};
use super::preservation::PreservationRules;
use super::semantic::{CompactionError, LlmClient, SemanticCompactor};
use super::structural::StructuralFilter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactedContext {
    pub system_prompt: String,
    pub messages: Vec<Message>,
    pub learnings: Vec<String>,
    pub preserved_facts: Vec<String>,
    pub token_count: usize,
}

pub struct CompactionPipeline<C: LlmClient> {
    budget: ContextBudget,
    preservation_rules: PreservationRules,
    semantic_compactor: SemanticCompactor<C>,
    light_every_turn: bool,
}

impl<C: LlmClient> CompactionPipeline<C> {
    pub fn new(
        budget: ContextBudget,
        preservation_rules: PreservationRules,
        client: C,
        light_every_turn: bool,
    ) -> Self {
        let semantic_compactor = SemanticCompactor::new(client, budget.clone());
        Self {
            budget,
            preservation_rules,
            semantic_compactor,
            light_every_turn,
        }
    }

    /// Create a pipeline from CompactionConfig-style values.
    pub fn from_config(
        max_tokens: usize,
        reserved_system: usize,
        reserved_response: usize,
        light_every_turn: bool,
        deep_trigger_pct: f32,
        deep_target_pct: f32,
        client: C,
    ) -> Result<Self, BudgetError> {
        let budget = ContextBudget::new(
            max_tokens,
            reserved_system,
            reserved_response,
            deep_trigger_pct,
            deep_target_pct,
        )?;

        Ok(Self::new(
            budget,
            PreservationRules::default_rules(),
            client,
            light_every_turn,
        ))
    }

    /// Run compaction on the given messages. This is the main entry point
    /// called after each agent turn.
    pub async fn compact(
        &self,
        system_prompt: &str,
        messages: &[Message],
    ) -> Result<CompactedContext, CompactionError> {
        let mut working_messages = messages.to_vec();

        // Step 1: Light compaction.
        if self.light_every_turn {
            working_messages = StructuralFilter::apply(&working_messages);
        }

        // Step 2: Deep-compaction budget check.
        let current_tokens =
            estimate_message_tokens(&working_messages) + estimate_tokens(system_prompt);

        if self.budget.needs_deep_compaction(current_tokens) {
            // Step 3: Extract preservation facts before deep compaction.
            let _ = self
                .preservation_rules
                .extract_preserved_facts(&working_messages);

            // Step 4: Deep compaction with graceful fallback.
            match self
                .semantic_compactor
                .compact(&working_messages, &self.preservation_rules)
                .await
            {
                Ok(compacted) => {
                    working_messages = compacted.messages;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "deep compaction failed, using light compaction only");
                }
            }
        }

        // Step 5: Build output from final state.
        let preserved_facts = self
            .preservation_rules
            .extract_preserved_facts(&working_messages);
        let learnings = Self::extract_learnings(&working_messages);
        let token_count =
            estimate_message_tokens(&working_messages) + estimate_tokens(system_prompt);

        Ok(CompactedContext {
            system_prompt: system_prompt.to_string(),
            messages: working_messages,
            learnings,
            preserved_facts,
            token_count,
        })
    }

    fn extract_learnings(messages: &[Message]) -> Vec<String> {
        const TOOL_ERROR_MARKERS: [&str; 5] = [
            "error",
            "failed",
            "exception",
            "not found",
            "permission denied",
        ];
        const LEARNING_PREFIX: &str = "LEARNING:";

        let mut learnings = Vec::new();

        for message in messages {
            if message.role.eq_ignore_ascii_case("tool") {
                let lowered = message.content.to_lowercase();
                if TOOL_ERROR_MARKERS
                    .iter()
                    .any(|marker| lowered.contains(marker))
                {
                    let normalized = normalize_whitespace(&message.content);
                    let snippet: String = normalized.chars().take(200).collect();
                    learnings.push(format!("Tool call failed: {snippet}"));
                }
            }

            if message.role.eq_ignore_ascii_case("assistant") {
                for line in message.content.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with(LEARNING_PREFIX) {
                        learnings.push(trimmed.to_string());
                    }
                }
            }
        }

        dedupe_preserving_order(learnings)
    }
}

fn normalize_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn dedupe_preserving_order(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();

    for item in items {
        let key = normalize_whitespace(&item).to_lowercase();
        if seen.insert(key) {
            deduped.push(item);
        }
    }

    deduped
}
