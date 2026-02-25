use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument};

use super::classify::{classify, ClassificationError, LlmClient};
use super::model_registry::ModelInfo;
use super::types::*;

/// What triggered a reclassification request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ReclassificationTrigger {
    /// User explicitly asked to switch modes or reclassify.
    UserRequested,
    /// The orchestrator/root agent detected a shift in work nature.
    OrchestratorDetected { reason: String },
}

/// A request to reclassify the current work into a potentially different mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReclassificationRequest {
    /// What triggered this reclassification.
    pub trigger: ReclassificationTrigger,
    /// The mode currently in use.
    pub current_mode: String,
    /// The current model in use.
    pub current_model: String,
    /// The latest user prompt or orchestrator summary of current work.
    pub prompt: String,
    /// Updated compacted context reflecting the conversation so far.
    pub updated_context: CompactedContext,
    /// Available modes to choose from.
    pub available_modes: Vec<ModeInfo>,
    /// Project context.
    pub project_context: ProjectContext,
}

/// Result of a reclassification attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReclassificationResult {
    /// The new classification output.
    pub new_output: RouterOutput,
    /// Whether the mode actually changed.
    pub mode_changed: bool,
    /// The previous mode (for event emission / logging).
    pub previous_mode: String,
    /// What triggered the reclassification.
    pub trigger: ReclassificationTrigger,
}

/// Reclassify the current work, potentially switching modes mid-run.
///
/// This function reuses the existing classify() function but wraps it
/// with reclassification-specific logic: it compares the new classification
/// against the current mode and reports whether a switch is needed.
#[instrument(skip(request, llm_client, available_models), fields(current_mode = %request.current_mode, trigger = ?request.trigger))]
pub fn reclassify(
    request: &ReclassificationRequest,
    llm_client: &dyn LlmClient,
    router_model: &str,
    available_models: &[ModelInfo],
) -> Result<ReclassificationResult, ClassificationError> {
    info!(current_mode = %request.current_mode, "reclassification attempt");

    let router_input = RouterInput {
        source: PromptSource::User,
        prompt: request.prompt.clone(),
        available_modes: request.available_modes.clone(),
        conversation_history: request.updated_context.clone(),
        project_context: request.project_context.clone(),
    };

    let new_output = classify(&router_input, llm_client, router_model, available_models)?;
    let mode_changed = new_output.mode != request.current_mode;

    debug!(
        new_mode = %new_output.mode,
        mode_changed,
        confidence = new_output.confidence,
        "reclassification result"
    );

    Ok(ReclassificationResult {
        new_output,
        mode_changed,
        previous_mode: request.current_mode.clone(),
        trigger: request.trigger.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt_router::classify::LlmClient;

    struct MockLlmClient {
        response: String,
    }

    impl LlmClient for MockLlmClient {
        fn complete(&self, _prompt: &str, _model: &str) -> Result<String, ClassificationError> {
            Ok(self.response.clone())
        }
    }

    fn sample_request(current_mode: &str) -> ReclassificationRequest {
        ReclassificationRequest {
            trigger: ReclassificationTrigger::UserRequested,
            current_mode: current_mode.to_string(),
            current_model: "claude-sonnet".to_string(),
            prompt: "Actually, let me just plan this out first".to_string(),
            updated_context: CompactedContext {
                messages_summary: "User started implementing auth but wants to rethink approach"
                    .to_string(),
                learnings: vec!["Project uses JWT".to_string()],
                preserved_facts: vec![],
                token_count: 120,
            },
            available_modes: vec![
                ModeInfo {
                    name: "Plan".to_string(),
                    description: "Structured decomposition".to_string(),
                },
                ModeInfo {
                    name: "Implement".to_string(),
                    description: "Code generation".to_string(),
                },
            ],
            project_context: ProjectContext {
                languages: vec!["Rust".to_string()],
                frameworks: vec!["Tauri".to_string()],
                file_structure_hints: vec![],
            },
        }
    }

    #[test]
    fn reclassify_detects_mode_change() {
        let request = sample_request("Implement");
        let llm = MockLlmClient {
            response: r#"{"mode":"Plan","model":"claude-sonnet","confidence":0.85}"#.to_string(),
        };

        let result = reclassify(&request, &llm, "router-model", &[]).unwrap();

        assert!(result.mode_changed);
        assert_eq!(result.previous_mode, "Implement");
        assert_eq!(result.new_output.mode, "Plan");
        assert_eq!(result.trigger, ReclassificationTrigger::UserRequested);
    }

    #[test]
    fn reclassify_detects_no_change() {
        let request = sample_request("Implement");
        let llm = MockLlmClient {
            response: r#"{"mode":"Implement","model":"claude-sonnet","confidence":0.9}"#
                .to_string(),
        };

        let result = reclassify(&request, &llm, "router-model", &[]).unwrap();

        assert!(!result.mode_changed);
        assert_eq!(result.previous_mode, "Implement");
        assert_eq!(result.new_output.mode, "Implement");
    }

    #[test]
    fn reclassify_propagates_llm_error() {
        struct FailingClient;
        impl LlmClient for FailingClient {
            fn complete(
                &self,
                _prompt: &str,
                _model: &str,
            ) -> Result<String, ClassificationError> {
                Err(ClassificationError {
                    message: "LLM unavailable".to_string(),
                })
            }
        }

        let request = sample_request("Plan");
        let result = reclassify(&request, &FailingClient, "router-model", &[]);

        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("LLM unavailable"));
    }

    #[test]
    fn reclassify_with_orchestrator_trigger() {
        let mut request = sample_request("Plan");
        request.trigger = ReclassificationTrigger::OrchestratorDetected {
            reason: "User shifted from planning to writing code".to_string(),
        };

        let llm = MockLlmClient {
            response: r#"{"mode":"Implement","model":"claude-sonnet","confidence":0.75}"#
                .to_string(),
        };

        let result = reclassify(&request, &llm, "router-model", &[]).unwrap();

        assert!(result.mode_changed);
        assert_eq!(
            result.trigger,
            ReclassificationTrigger::OrchestratorDetected {
                reason: "User shifted from planning to writing code".to_string(),
            }
        );
    }
}
