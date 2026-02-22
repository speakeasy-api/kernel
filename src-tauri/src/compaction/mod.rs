pub mod budget;
pub mod pipeline;
pub mod preservation;
pub mod semantic;
pub mod structural;

#[cfg(test)]
mod tests;

pub use budget::{estimate_message_tokens, estimate_tokens, BudgetError, ContextBudget, Message};
pub use pipeline::{CompactedContext, CompactionPipeline};
pub use preservation::{PreservationRules, PreservedPattern};
pub use semantic::{CompactionError, LlmClient, SemanticCompactor};
pub use structural::StructuralFilter;
