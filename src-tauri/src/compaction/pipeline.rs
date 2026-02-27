use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

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
    #[serde(default)]
    pub pinned_snippets: Vec<PinnedSnippet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinnedSnippet {
    pub ordinal: i64,
    pub snippet: String,
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
    #[instrument(skip_all, fields(message_count = messages.len()))]
    pub async fn compact(
        &self,
        system_prompt: &str,
        messages: &[Message],
    ) -> Result<CompactedContext, CompactionError> {
        info!(
            message_count = messages.len(),
            "starting compaction pipeline"
        );
        let mut working_messages = messages.to_vec();

        // Step 1: Light compaction.
        if self.light_every_turn {
            debug!("applying light compaction (structural filters)");
            working_messages = StructuralFilter::apply(&working_messages);
            debug!(
                after_light = working_messages.len(),
                "light compaction done"
            );
        }

        // Step 1b: Partition pinned vs unpinned messages.
        let (pinned, unpinned): (Vec<Message>, Vec<Message>) =
            working_messages.into_iter().partition(|m| m.pinned);

        // Separate newly pinned (context_snippet is None) from already-processed
        let newly_pinned: Vec<(usize, &Message)> = pinned
            .iter()
            .enumerate()
            .filter(|(_, m)| m.context_snippet.is_none())
            .map(|(i, m)| (i, m))
            .collect();

        debug!(
            pinned_count = pinned.len(),
            newly_pinned_count = newly_pinned.len(),
            unpinned_count = unpinned.len(),
            "partitioned pinned/unpinned messages"
        );

        working_messages = unpinned;

        // Step 2: Deep-compaction budget check (only on unpinned).
        let current_tokens =
            estimate_message_tokens(&working_messages) + estimate_tokens(system_prompt);
        debug!(current_tokens, "checking deep compaction budget");

        let mut pinned_snippets = Vec::new();

        if self.budget.needs_deep_compaction(current_tokens) {
            // Step 3: Extract preservation facts before deep compaction.
            debug!("extracting preservation facts");
            let _ = self
                .preservation_rules
                .extract_preserved_facts(&working_messages);

            // Step 4: Deep compaction with graceful fallback.
            debug!("running semantic deep compaction");
            match self
                .semantic_compactor
                .compact_with_pinned(&working_messages, &self.preservation_rules, &newly_pinned)
                .await
            {
                Ok(compacted) => {
                    debug!(
                        compacted_messages = compacted.messages.len(),
                        "deep compaction succeeded"
                    );
                    working_messages = compacted.messages;
                    pinned_snippets = compacted.pinned_snippets;
                }
                Err(e) => {
                    warn!(error = %e, "deep compaction failed, using light compaction only");
                }
            }
        } else {
            debug!(current_tokens, "below deep compaction trigger, skipping");
        }

        // Step 5: Append pinned messages back after compacted summary.
        working_messages.extend(pinned);

        // Step 6: Build output from final state.
        debug!("building compaction output");
        let preserved_facts = self
            .preservation_rules
            .extract_preserved_facts(&working_messages);
        let learnings = Self::extract_learnings(&working_messages);
        let token_count =
            estimate_message_tokens(&working_messages) + estimate_tokens(system_prompt);

        info!(
            before_messages = messages.len(),
            after_messages = working_messages.len(),
            token_count,
            learnings = learnings.len(),
            preserved_facts = preserved_facts.len(),
            "compaction pipeline complete"
        );

        Ok(CompactedContext {
            system_prompt: system_prompt.to_string(),
            messages: working_messages,
            learnings,
            preserved_facts,
            token_count,
            pinned_snippets,
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
