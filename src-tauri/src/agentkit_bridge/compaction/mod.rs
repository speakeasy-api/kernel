mod backend;
mod strategies;
mod triggers;

pub use backend::KernelCompactionBackend;
pub use strategies::{PersistSnapshotStrategy, PreservePinnedStrategy, TruncateToolResultsStrategy};
pub use triggers::{AnyOfTrigger, LargeToolResultTrigger, TokenBudgetTrigger};

/// Metadata key used by `PreservePinnedStrategy` to forward the pinned items
/// to the backend (so it can generate context snippets for them).
pub const META_PINNED_ITEMS: &str = "kernel.compaction.pinned_items";
