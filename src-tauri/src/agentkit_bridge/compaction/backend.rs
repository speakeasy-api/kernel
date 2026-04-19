use std::sync::Arc;

use agentkit::compaction::{
    CompactionBackend, CompactionError, SummaryRequest, SummaryResult,
};
use agentkit::core::{Item, ItemKind, MetadataMap, Part, TextPart, TurnCancellation};
use async_trait::async_trait;
use serde_json::Value;
use sqlx::SqlitePool;
use tracing::{info, warn};

use crate::agentkit_bridge::convert::{item_ordinal, items_to_messages, messages_to_compaction};
use crate::anthropic::client::LlmClient2;
use crate::compaction::{
    self, build_compaction_prompt, parse_compactor_response, PinnedReference, PreservationRules,
};

use super::META_PINNED_ITEMS;

/// Backend that drives semantic compaction via the kernel LLM client and
/// persists derived state (pinned context snippets, learnings) to the DB.
pub struct KernelCompactionBackend {
    client: Arc<LlmClient2>,
    model: String,
    pool: SqlitePool,
    session_id: String,
    /// Soft target tokens for the LLM to aim at when summarising.
    target_tokens: usize,
    preservation: PreservationRules,
}

impl KernelCompactionBackend {
    pub fn new(
        client: Arc<LlmClient2>,
        model: String,
        pool: SqlitePool,
        session_id: String,
        target_tokens: usize,
    ) -> Self {
        Self {
            client,
            model,
            pool,
            session_id,
            target_tokens,
            preservation: PreservationRules::default_rules(),
        }
    }
}

#[async_trait]
impl CompactionBackend for KernelCompactionBackend {
    async fn summarize(
        &self,
        request: SummaryRequest,
        cancellation: Option<TurnCancellation>,
    ) -> Result<SummaryResult, CompactionError> {
        // Fast path: if we've already been cancelled by the time the strategy
        // invokes the backend, skip the LLM call entirely. This also prevents
        // `PersistSnapshotStrategy` from committing a snapshot the user no
        // longer wants — it short-circuits on our `Err` before the DB write.
        if let Some(token) = cancellation.as_ref() {
            if token.is_cancelled() {
                return Err(CompactionError::Cancelled);
            }
        }

        // Decode pinned items (if any) supplied by `PreservePinnedStrategy`.
        let pinned_items: Vec<Item> = request
            .metadata
            .get(META_PINNED_ITEMS)
            .and_then(|v| serde_json::from_value::<Vec<Item>>(v.clone()).ok())
            .unwrap_or_default();

        // Newly pinned = pinned items that don't yet have a context snippet.
        let newly_pinned: Vec<&Item> = pinned_items
            .iter()
            .filter(|item| {
                !item
                    .metadata
                    .contains_key(crate::agentkit_bridge::convert::META_CONTEXT_SNIPPET)
            })
            .collect();

        let messages = items_to_messages(&request.items);
        let compact_msgs = messages_to_compaction(&messages);
        let preserved_facts = self.preservation.extract_preserved_facts(&compact_msgs);

        // Build PinnedReference list ordered by index so the LLM's `position`
        // field maps back to `newly_pinned[position]` -> ordinal.
        let mut pinned_refs: Vec<PinnedReference> = Vec::new();
        let mut newly_pinned_ordinals: Vec<Option<i64>> = Vec::new();
        for (position, item) in newly_pinned.iter().enumerate() {
            let ordinal = item_ordinal(item);
            newly_pinned_ordinals.push(ordinal);

            // Convert this single item to messages so role/content align.
            let single = std::slice::from_ref(*item);
            let single_msgs = items_to_messages(single);
            let (role_str, content_str) = single_msgs
                .first()
                .map(|m| {
                    let role = match m.role {
                        crate::anthropic::types::Role::User => "user",
                        crate::anthropic::types::Role::Assistant => "assistant",
                    };
                    (role.to_string(), summarise_content(&m.content))
                })
                .unwrap_or_else(|| ("user".to_string(), String::new()));

            pinned_refs.push(PinnedReference {
                position,
                role: role_str,
                content: content_str,
            });
        }

        let (system_prompt, user_prompt) = build_compaction_prompt(
            &compact_msgs,
            self.target_tokens,
            &preserved_facts,
            &pinned_refs,
        );

        // Race the summarizer LLM call against the cancellation token so that
        // pressing Esc during compaction aborts promptly rather than waiting
        // for the provider to respond. Returning `Cancelled` here causes
        // `PersistSnapshotStrategy` to skip the snapshot write.
        let summarize_call =
            self.client
                .complete_system_async(&system_prompt, &user_prompt, &self.model, 4096);
        let response = match cancellation.as_ref() {
            Some(token) => tokio::select! {
                biased;
                _ = token.cancelled() => return Err(CompactionError::Cancelled),
                res = summarize_call => {
                    res.map_err(|e| CompactionError::Failed(format!("LLM call: {e}")))?
                }
            },
            None => summarize_call
                .await
                .map_err(|e| CompactionError::Failed(format!("LLM call: {e}")))?,
        };

        let parsed = parse_compactor_response(&response)
            .map_err(|e| CompactionError::Failed(format!("parse: {e}")))?;

        // Persist newly generated pinned context snippets.
        if let Some(snippets) = parsed.pinned_snippets.as_ref() {
            for snippet in snippets {
                let Some(ordinal) = newly_pinned_ordinals
                    .get(snippet.position)
                    .and_then(|o| *o)
                else {
                    warn!(
                        position = snippet.position,
                        "pinned snippet position has no DB ordinal — skipping"
                    );
                    continue;
                };
                if let Err(e) = crate::db::queries::update_context_snippet(
                    &self.pool,
                    &self.session_id,
                    ordinal,
                    &snippet.snippet,
                )
                .await
                {
                    warn!(error = %e, ordinal, "failed to persist context snippet");
                }
            }
        }

        // Persist learnings as a structured event for resume / UI display.
        if !parsed.learnings.is_empty() || !parsed.preserved_facts.is_empty() {
            let event_data = serde_json::json!({
                "learnings": parsed.learnings,
                "preserved_facts": parsed.preserved_facts,
            })
            .to_string();
            let _ = crate::db::queries::insert_event(
                &self.pool,
                &self.session_id,
                None,
                "CompactionLearnings",
                &event_data,
            )
            .await;
        }

        // Build the summary text from the parsed messages.
        let summary_text = parsed
            .messages
            .iter()
            .map(|m| format!("[{}]: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n\n");

        let summary_token_estimate = compaction::estimate_tokens(&summary_text);
        info!(
            session_id = %self.session_id,
            input_messages = messages.len(),
            output_messages = parsed.messages.len(),
            summary_tokens = summary_token_estimate,
            target_tokens = self.target_tokens,
            "compaction backend produced summary"
        );

        let mut metadata = MetadataMap::new();
        metadata.insert(
            "kernel.compaction.summary_tokens".into(),
            Value::from(summary_token_estimate as u64),
        );
        metadata.insert(
            "kernel.compaction.input_message_count".into(),
            Value::from(messages.len() as u64),
        );

        let summary_item = Item::new(
            ItemKind::Context,
            vec![Part::Text(TextPart {
                text: format!("[Compacted summary]\n\n{summary_text}"),
                metadata: Default::default(),
            })],
        );

        Ok(SummaryResult {
            items: vec![summary_item],
            metadata,
        })
    }
}

fn summarise_content(blocks: &[crate::anthropic::types::ContentBlock]) -> String {
    use crate::anthropic::types::ContentBlock;
    blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => text.clone(),
            ContentBlock::ToolUse { name, .. } => format!("[tool_use: {name}]"),
            ContentBlock::ToolResult { content, .. } => content.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
