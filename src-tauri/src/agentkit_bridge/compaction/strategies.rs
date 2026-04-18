use std::sync::Arc;

use agentkit::compaction::{
    CompactionContext, CompactionError, CompactionRequest, CompactionResult, CompactionStrategy,
};
use agentkit::core::{Item, ItemKind, Part, ToolOutput};
use async_trait::async_trait;
use sqlx::SqlitePool;
use tracing::{info, warn};

use crate::agentkit_bridge::convert::{item_pinned, items_to_messages};

use super::META_PINNED_ITEMS;

/// Truncate large tool results to head + tail in place. Mirrors the kernel
/// `light_compact` behaviour. Doesn't change item count.
pub struct TruncateToolResultsStrategy {
    pub max_chars: usize,
    pub head: usize,
    pub tail: usize,
}

impl Default for TruncateToolResultsStrategy {
    fn default() -> Self {
        Self {
            max_chars: 2000,
            head: 500,
            tail: 200,
        }
    }
}

#[async_trait]
impl CompactionStrategy for TruncateToolResultsStrategy {
    async fn apply(
        &self,
        request: CompactionRequest,
        _ctx: &mut CompactionContext<'_>,
    ) -> Result<CompactionResult, CompactionError> {
        let mut transcript = request.transcript;
        let mut replaced = 0usize;
        for item in transcript.iter_mut() {
            if item.kind != ItemKind::Tool {
                continue;
            }
            for part in item.parts.iter_mut() {
                if let Part::ToolResult(tr) = part {
                    if let ToolOutput::Text(text) = &mut tr.output {
                        let char_count = text.chars().count();
                        if char_count > self.max_chars {
                            let head: String = text.chars().take(self.head).collect();
                            let tail: String = text.chars().skip(char_count - self.tail).collect();
                            *text = format!(
                                "{head}\n... [truncated, {char_count} chars total] ...\n{tail}"
                            );
                            replaced += 1;
                        }
                    }
                }
            }
        }
        Ok(CompactionResult::new(transcript, replaced))
    }
}

/// Wrap an inner strategy so that pinned items (those stamped with
/// `kernel.pinned = true` via `convert::messages_to_items_with_meta`) are
/// withheld from the inner strategy and re-appended verbatim at the end.
///
/// The pinned items are also serialised into request metadata under
/// [`META_PINNED_ITEMS`] so the backend (e.g. `KernelCompactionBackend`)
/// can use them as reference for snippet generation.
pub struct PreservePinnedStrategy {
    inner: Arc<dyn CompactionStrategy>,
}

impl PreservePinnedStrategy {
    pub fn wrap(inner: impl CompactionStrategy + 'static) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }
}

#[async_trait]
impl CompactionStrategy for PreservePinnedStrategy {
    async fn apply(
        &self,
        request: CompactionRequest,
        ctx: &mut CompactionContext<'_>,
    ) -> Result<CompactionResult, CompactionError> {
        let CompactionRequest {
            session_id,
            turn_id,
            transcript,
            reason,
            mut metadata,
        } = request;

        let (pinned, unpinned): (Vec<Item>, Vec<Item>) =
            transcript.into_iter().partition(item_pinned);

        if pinned.is_empty() {
            let request = CompactionRequest {
                session_id,
                turn_id,
                transcript: unpinned,
                reason,
                metadata,
            };
            return self.inner.apply(request, ctx).await;
        }

        match serde_json::to_value(&pinned) {
            Ok(value) => {
                metadata.insert(META_PINNED_ITEMS.to_string(), value);
            }
            Err(e) => {
                warn!(error = %e, "failed to serialise pinned items for backend reference");
            }
        }

        let inner_request = CompactionRequest {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            transcript: unpinned,
            reason: reason.clone(),
            metadata,
        };
        let mut result = self.inner.apply(inner_request, ctx).await?;

        result.transcript.extend(pinned);
        Ok(result)
    }
}

/// Wraps an inner strategy. After it runs, persists the resulting transcript
/// as a kernel snapshot (`context_snapshots` row + `ContextCompacted` event)
/// when at least one item was actually replaced. Acts as the outermost layer
/// so it sees both the original transcript and the post-compaction one.
pub struct PersistSnapshotStrategy {
    inner: Arc<dyn CompactionStrategy>,
    pool: SqlitePool,
    session_id: String,
}

impl PersistSnapshotStrategy {
    pub fn wrap(
        inner: impl CompactionStrategy + 'static,
        pool: SqlitePool,
        session_id: String,
    ) -> Self {
        Self {
            inner: Arc::new(inner),
            pool,
            session_id,
        }
    }
}

#[async_trait]
impl CompactionStrategy for PersistSnapshotStrategy {
    async fn apply(
        &self,
        request: CompactionRequest,
        ctx: &mut CompactionContext<'_>,
    ) -> Result<CompactionResult, CompactionError> {
        let original_len = request.transcript.len();
        let result = self.inner.apply(request, ctx).await?;

        if result.replaced_items == 0 {
            return Ok(result);
        }

        let messages = items_to_messages(&result.transcript);
        let summary_json = serde_json::to_string(&messages)
            .map_err(|e| CompactionError::Failed(format!("serialise snapshot: {e}")))?;

        let max_ord = crate::db::queries::get_max_ordinal(&self.pool, &self.session_id)
            .await
            .map_err(|e| CompactionError::Failed(format!("get_max_ordinal: {e}")))?
            .unwrap_or(0);

        crate::db::queries::save_context_snapshot(
            &self.pool,
            &self.session_id,
            max_ord,
            &summary_json,
        )
        .await
        .map_err(|e| CompactionError::Failed(format!("save_context_snapshot: {e}")))?;

        let event_data = serde_json::json!({
            "before_messages": original_len,
            "after_messages": result.transcript.len(),
            "replaced_items": result.replaced_items,
            "up_to_ordinal": max_ord,
        })
        .to_string();
        let _ = crate::db::queries::insert_event(
            &self.pool,
            &self.session_id,
            None,
            "ContextCompacted",
            &event_data,
        )
        .await;

        info!(
            session_id = %self.session_id,
            up_to_ordinal = max_ord,
            before = original_len,
            after = result.transcript.len(),
            "context snapshot persisted"
        );

        Ok(result)
    }
}
