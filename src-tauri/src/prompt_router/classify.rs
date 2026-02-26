use tracing::{debug, info, instrument, warn};

use crate::prompt_router::model_registry::ModelInfo;
use crate::prompt_router::{ModeInfo, RouterInput, RouterOutput};

#[derive(Debug)]
pub struct ClassificationError {
    pub message: String,
}

impl std::fmt::Display for ClassificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Classification error: {}", self.message)
    }
}

impl std::error::Error for ClassificationError {}

/// Trait for making LLM completion calls. Implementations will wrap actual
/// provider APIs (OpenAI, Anthropic, etc.)
pub trait LlmClient: Send + Sync {
    fn complete(&self, prompt: &str, model: &str) -> Result<String, ClassificationError>;
}

/// Build the classification prompt using mode metadata and compacted context.
///
/// When `available_models` is non-empty the prompt constrains the LLM to pick
/// one of the listed model IDs. When empty (cold cache) it defaults to the
/// fallback model.
pub fn build_classification_prompt(input: &RouterInput, available_models: &[ModelInfo]) -> String {
    let modes = if input.available_modes.is_empty() {
        "- (none provided)".to_string()
    } else {
        input
            .available_modes
            .iter()
            .map(|mode| format!("- {}: {}", mode.name, mode.description))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let languages = list_or_none(&input.project_context.languages);
    let frameworks = list_or_none(&input.project_context.frameworks);
    let file_structure = list_or_none(&input.project_context.file_structure_hints);

    let (task_desc, model_section, response_format) = if available_models.is_empty() {
        (
            "select the best mode",
            String::new(),
            r#"{"mode": "<mode_name>", "confidence": <0.0-1.0>}"#,
        )
    } else {
        let list = available_models
            .iter()
            .map(|m| {
                let ctx = if m.context_length > 0 {
                    format!(" ({}k context)", m.context_length / 1000)
                } else {
                    String::new()
                };
                format!("- {}: {}{}", m.id, m.name, ctx)
            })
            .collect::<Vec<_>>()
            .join("\n");
        (
            "select the best mode and model",
            format!(
                "Available models (you MUST pick one of these IDs exactly):\n{}\n\n",
                list
            ),
            r#"{"mode": "<mode_name>", "model": "<model_id_from_list>", "confidence": <0.0-1.0>}"#,
        )
    };

    format!(
        "You are a prompt router. Given the user's prompt and project context, {task_desc}.\n\n\
Available modes:\n\
{modes}\n\n\
{model_section}\
Project context:\n\
Languages: [{languages}]\n\
Frameworks: [{frameworks}]\n\
File structure: [{file_structure}]\n\n\
Conversation so far: {summary}\n\n\
User prompt: {prompt}\n\n\
Respond with ONLY a JSON object:\n\
{response_format}",
        task_desc = task_desc,
        modes = modes,
        model_section = model_section,
        languages = languages,
        frameworks = frameworks,
        file_structure = file_structure,
        summary = input.conversation_history.messages_summary,
        prompt = input.prompt,
        response_format = response_format,
    )
}

#[instrument(skip(input, llm_client, available_models), fields(router_model, prompt_len = input.prompt.len()))]
pub fn classify(
    input: &RouterInput,
    llm_client: &dyn LlmClient,
    router_model: &str,
    available_models: &[ModelInfo],
) -> Result<RouterOutput, ClassificationError> {
    info!(
        prompt_len = input.prompt.len(),
        modes = input.available_modes.len(),
        models = available_models.len(),
        "classifying prompt"
    );
    let prompt = build_classification_prompt(input, available_models);
    let response = llm_client.complete(&prompt, router_model)?;
    let result = parse_classification_response(&response, &input.available_modes)?;
    debug!(
        mode = %result.mode,
        model = ?result.model,
        confidence = result.confidence,
        "classification result"
    );
    Ok(result)
}

#[instrument(skip(response, available_modes))]
pub fn parse_classification_response(
    response: &str,
    available_modes: &[ModeInfo],
) -> Result<RouterOutput, ClassificationError> {
    let trimmed = response.trim();

    let mut output = match serde_json::from_str::<RouterOutput>(trimmed) {
        Ok(parsed) => parsed,
        Err(primary_err) => {
            let first_brace = trimmed.find('{');
            let last_brace = trimmed.rfind('}');

            if let (Some(start), Some(end)) = (first_brace, last_brace) {
                if start <= end {
                    let candidate = &trimmed[start..=end];
                    serde_json::from_str::<RouterOutput>(candidate).map_err(|fallback_err| {
                        ClassificationError {
                            message: format!(
                                "failed to parse classification response as JSON (direct parse: {}; extracted parse: {}). raw response: {}",
                                primary_err, fallback_err, response
                            ),
                        }
                    })?
                } else {
                    return Err(ClassificationError {
                        message: format!(
                            "failed to parse classification response as JSON (direct parse: {}). raw response: {}",
                            primary_err, response
                        ),
                    });
                }
            } else {
                return Err(ClassificationError {
                    message: format!(
                        "failed to parse classification response as JSON (direct parse: {}). raw response: {}",
                        primary_err, response
                    ),
                });
            }
        }
    };

    if !available_modes.iter().any(|mode| mode.name == output.mode) {
        return Err(ClassificationError {
            message: format!(
                "classifier returned unknown mode '{}'. available modes: [{}]",
                output.mode,
                available_modes
                    .iter()
                    .map(|mode| mode.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }

    if !(0.0..=1.0).contains(&output.confidence) {
        warn!(
            confidence = output.confidence,
            "classifier confidence out of range, clamping"
        );
        output.confidence = output.confidence.clamp(0.0, 1.0);
    }

    Ok(output)
}

fn list_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt_router::{CompactedContext, ProjectContext, PromptSource, RouterInput};

    struct MockLlmClient {
        response: String,
    }

    impl LlmClient for MockLlmClient {
        fn complete(&self, _prompt: &str, _model: &str) -> Result<String, ClassificationError> {
            Ok(self.response.clone())
        }
    }

    fn sample_input() -> RouterInput {
        RouterInput {
            source: PromptSource::User,
            prompt: "Implement auth middleware".to_string(),
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
            conversation_history: CompactedContext {
                messages_summary: "User is building a Rust web service".to_string(),
                learnings: vec![],
                preserved_facts: vec![],
                token_count: 42,
            },
            project_context: ProjectContext {
                languages: vec!["Rust".to_string(), "TypeScript".to_string()],
                frameworks: vec!["Tauri".to_string()],
                file_structure_hints: vec!["src-tauri/src/".to_string(), "src/".to_string()],
            },
        }
    }

    #[test]
    fn build_prompt_contains_required_sections() {
        let input = sample_input();
        let prompt = build_classification_prompt(&input, &[]);

        assert!(prompt.contains("Available modes:"));
        assert!(prompt.contains("- Plan: Structured decomposition"));
        assert!(prompt.contains("- Implement: Code generation"));
        assert!(prompt.contains("Project context:"));
        assert!(prompt.contains("Languages: [Rust, TypeScript]"));
        assert!(prompt.contains("Frameworks: [Tauri]"));
        assert!(prompt.contains("File structure: [src-tauri/src/, src/]"));
        assert!(prompt.contains("Conversation so far: User is building a Rust web service"));
        assert!(prompt.contains("User prompt: Implement auth middleware"));
    }

    #[test]
    fn build_prompt_empty_models_omits_model_section() {
        let input = sample_input();
        let prompt = build_classification_prompt(&input, &[]);
        assert!(prompt.contains("select the best mode."));
        assert!(!prompt.contains("model"));
        assert!(!prompt.contains("anthropic/claude-sonnet-4-6"));
    }

    #[test]
    fn build_prompt_with_models_lists_ids() {
        let input = sample_input();
        let models = vec![
            ModelInfo {
                id: "anthropic/claude-sonnet-4-6".into(),
                name: "Claude Sonnet 4.6".into(),
                description: "".into(),
                context_length: 200_000,
            },
            ModelInfo {
                id: "google/gemini-2.5-pro".into(),
                name: "Gemini 2.5 Pro".into(),
                description: "".into(),
                context_length: 1_000_000,
            },
        ];
        let prompt = build_classification_prompt(&input, &models);
        assert!(prompt.contains("select the best mode and model."));
        assert!(prompt.contains("Available models (you MUST pick one of these IDs exactly):"));
        assert!(prompt.contains("- anthropic/claude-sonnet-4-6: Claude Sonnet 4.6 (200k context)"));
        assert!(prompt.contains("- google/gemini-2.5-pro: Gemini 2.5 Pro (1000k context)"));
        assert!(prompt.contains("model_id_from_list"));
    }

    #[test]
    fn parse_clean_json() {
        let modes = vec![ModeInfo {
            name: "Plan".to_string(),
            description: "Structured decomposition".to_string(),
        }];

        let result = parse_classification_response(
            r#"{"mode":"Plan","model":"claude-sonnet","confidence":0.75}"#,
            &modes,
        )
        .unwrap();

        assert_eq!(result.mode, "Plan");
        assert_eq!(result.model.as_deref(), Some("claude-sonnet"));
        assert_eq!(result.confidence, 0.75);
    }

    #[test]
    fn parse_json_with_surrounding_text() {
        let modes = vec![ModeInfo {
            name: "Implement".to_string(),
            description: "Code generation".to_string(),
        }];

        let result = parse_classification_response(
            "Here is your answer:\n{\"mode\":\"Implement\",\"model\":\"gpt-4.1\",\"confidence\":0.8}\nThanks!",
            &modes,
        )
        .unwrap();

        assert_eq!(result.mode, "Implement");
        assert_eq!(result.model.as_deref(), Some("gpt-4.1"));
        assert_eq!(result.confidence, 0.8);
    }

    #[test]
    fn parse_json_without_model_field() {
        let modes = vec![ModeInfo {
            name: "Plan".to_string(),
            description: "Structured decomposition".to_string(),
        }];

        let result = parse_classification_response(
            r#"{"mode":"Plan","confidence":0.9}"#,
            &modes,
        )
        .unwrap();

        assert_eq!(result.mode, "Plan");
        assert_eq!(result.model, None);
        assert_eq!(result.confidence, 0.9);
    }

    #[test]
    fn parse_invalid_response_returns_error() {
        let modes = vec![ModeInfo {
            name: "Plan".to_string(),
            description: "Structured decomposition".to_string(),
        }];

        let err = parse_classification_response("not json", &modes).unwrap_err();
        assert!(err.message.contains("raw response: not json"));
    }

    #[test]
    fn rejects_unknown_mode() {
        let modes = vec![ModeInfo {
            name: "Plan".to_string(),
            description: "Structured decomposition".to_string(),
        }];

        let err = parse_classification_response(
            r#"{"mode":"Debug","model":"claude","confidence":0.5}"#,
            &modes,
        )
        .unwrap_err();

        assert!(err.message.contains("unknown mode 'Debug'"));
    }

    #[test]
    fn clamps_confidence_above_one() {
        let modes = vec![ModeInfo {
            name: "Plan".to_string(),
            description: "Structured decomposition".to_string(),
        }];

        let result = parse_classification_response(
            r#"{"mode":"Plan","model":"claude","confidence":1.5}"#,
            &modes,
        )
        .unwrap();

        assert_eq!(result.confidence, 1.0);
    }

    #[test]
    fn classify_calls_client_and_parses_result() {
        let input = sample_input();
        let llm = MockLlmClient {
            response: r#"{"mode":"Plan","model":"claude-3-7-sonnet","confidence":0.6}"#.to_string(),
        };

        let result = classify(&input, &llm, "router-model", &[]).unwrap();
        assert_eq!(result.mode, "Plan");
        assert_eq!(result.model.as_deref(), Some("claude-3-7-sonnet"));
        assert_eq!(result.confidence, 0.6);
    }
}
