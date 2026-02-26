use std::collections::HashSet;
use tracing::{debug, info, instrument, warn};

use super::classify::{classify, ClassificationError, LlmClient};
use super::model_registry::{ModelInfo, FALLBACK_MODEL};
use super::reclassify::{reclassify, ReclassificationRequest};
use super::types::*;
use super::user_override::{apply_override, ModeOverriddenEvent, OverrideError};

/// Trait for emitting prompt router events. Implementors connect to the
/// actual event system (spec 02). This decouples dispatch from the event store.
pub trait RouterEventSink: Send + Sync {
    /// Emit a prompt_classified event.
    fn emit_prompt_classified(&self, session_id: &str, output: &RouterOutput);

    /// Emit a mode_overridden event.
    fn emit_mode_overridden(&self, session_id: &str, event: &ModeOverriddenEvent);
}

/// Trait for loading full mode data. Implementors connect to the modes
/// system (spec 05). The router only sees ModeInfo (name + description);
/// this trait loads the full system prompt and tool permissions.
pub trait ModeLoader: Send + Sync {
    fn load_mode(&self, mode_name: &str) -> Result<LoadedMode, DispatchError>;
}

/// A fully loaded mode with its system prompt and tool permissions.
#[derive(Debug, Clone)]
pub struct LoadedMode {
    pub name: String,
    pub system_prompt: String,
    pub default_model: Option<String>,
    pub allowed_tools: Vec<String>,
}

/// The payload prepared by dispatch for the agent system to create
/// an entrypoint agent.
#[derive(Debug, Clone)]
pub struct AgentHandoff {
    /// The classified/overridden mode name.
    pub mode_name: String,
    /// The full system prompt for the selected mode.
    pub system_prompt: String,
    /// The model to use for the entrypoint agent.
    pub model: String,
    /// The compacted context to seed the agent with.
    pub context: CompactedContext,
    /// Tools the agent is allowed to use.
    pub allowed_tools: Vec<String>,
    /// The original user prompt.
    pub prompt: String,
    /// Classification confidence (1.0 for overrides).
    pub confidence: f32,
}

#[derive(Debug)]
pub enum DispatchError {
    Classification(ClassificationError),
    Override(OverrideError),
    ModeNotFound(String),
    ModelUnavailable {
        requested: String,
        fallback: Option<String>,
    },
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Classification(e) => write!(f, "Classification failed: {}", e),
            Self::Override(e) => write!(f, "Override failed: {}", e),
            Self::ModeNotFound(name) => write!(f, "Mode not found: {}", name),
            Self::ModelUnavailable {
                requested,
                fallback,
            } => write!(
                f,
                "Model '{}' unavailable, fallback: {:?}",
                requested, fallback
            ),
        }
    }
}

impl std::error::Error for DispatchError {}

impl From<ClassificationError> for DispatchError {
    fn from(e: ClassificationError) -> Self {
        Self::Classification(e)
    }
}

impl From<OverrideError> for DispatchError {
    fn from(e: OverrideError) -> Self {
        Self::Override(e)
    }
}

/// Resolve the model to use.
///
/// Priority: router-selected (validated against catalog) → mode default →
/// config default → [`FALLBACK_MODEL`].
///
/// Router-selected models are validated; mode and config defaults are trusted
/// as-is since they were explicitly configured by the user.
#[instrument(skip(loaded_mode, known_model_ids))]
fn resolve_model(
    router_model: Option<&str>,
    loaded_mode: &LoadedMode,
    known_model_ids: &HashSet<String>,
    config_default: Option<&str>,
) -> String {
    // 1. Try the router-selected model (validated against catalog)
    if let Some(m) = router_model {
        if !m.is_empty() {
            if known_model_ids.contains(m) {
                debug!(model = %m, "router model validated against catalog");
                return m.to_string();
            }
            debug!(model = %m, "router model not in catalog, trying defaults");
        }
    }

    // 2. Fall back to the mode's configured default — pass through as-is
    if let Some(ref default) = loaded_mode.default_model {
        if !default.is_empty() {
            debug!(model = %default, "using mode default model");
            return default.clone();
        }
    }

    // 3. Fall back to the global config default — pass through as-is
    if let Some(default) = config_default {
        if !default.is_empty() {
            debug!(model = %default, "using config default model");
            return default.to_string();
        }
    }

    // 4. Nothing configured — use hardcoded fallback
    debug!(
        fallback = FALLBACK_MODEL,
        "no candidate available, using fallback"
    );
    FALLBACK_MODEL.to_string()
}

/// Route a prompt through classification (or override) and prepare
/// the handoff payload for the agent system.
///
/// This is the primary entry point for the prompt router module.
#[instrument(
    skip(
        input,
        llm_client,
        mode_loader,
        event_sink,
        available_models,
        known_model_ids
    ),
    fields(session_id, user_override)
)]
pub fn dispatch(
    input: &RouterInput,
    user_override: Option<&str>,
    llm_client: &dyn LlmClient,
    router_model: &str,
    mode_loader: &dyn ModeLoader,
    event_sink: &dyn RouterEventSink,
    session_id: &str,
    available_models: &[ModelInfo],
    known_model_ids: &HashSet<String>,
    config_default_model: Option<&str>,
) -> Result<AgentHandoff, DispatchError> {
    info!(session_id, override_mode = ?user_override, "dispatching prompt");
    // Step 1: Classify or Override
    let output = if let Some(override_mode) = user_override {
        let default_output = RouterOutput {
            mode: "none".to_string(),
            model: Some(router_model.to_string()),
            confidence: 0.0,
        };
        let (overridden, event) =
            apply_override(&default_output, override_mode, None, &input.available_modes)?;
        event_sink.emit_mode_overridden(session_id, &event);
        overridden
    } else {
        let classified = classify(input, llm_client, router_model, available_models)?;
        event_sink.emit_prompt_classified(session_id, &classified);
        classified
    };

    // Step 2: Load the selected mode
    let loaded_mode = mode_loader.load_mode(&output.mode)?;

    // Step 3: Resolve the model against the full catalog
    let model = resolve_model(
        output.model.as_deref(),
        &loaded_mode,
        known_model_ids,
        config_default_model,
    );

    // Step 4: Build AgentHandoff
    debug!(
        mode = %output.mode,
        model = %model,
        confidence = output.confidence,
        tools = loaded_mode.allowed_tools.len(),
        "dispatch handoff prepared"
    );
    Ok(AgentHandoff {
        mode_name: output.mode,
        system_prompt: loaded_mode.system_prompt,
        model,
        context: input.conversation_history.clone(),
        allowed_tools: loaded_mode.allowed_tools,
        prompt: input.prompt.clone(),
        confidence: output.confidence,
    })
}

/// Dispatch a reclassification request. If the mode changes, loads the
/// new mode and prepares a fresh handoff.
///
/// Returns `Some(AgentHandoff)` if the mode changed, `None` if it stayed the same.
#[instrument(
    skip(
        request,
        llm_client,
        mode_loader,
        event_sink,
        available_models,
        known_model_ids
    ),
    fields(session_id)
)]
pub fn dispatch_reclassification(
    request: &ReclassificationRequest,
    llm_client: &dyn LlmClient,
    router_model: &str,
    mode_loader: &dyn ModeLoader,
    event_sink: &dyn RouterEventSink,
    session_id: &str,
    available_models: &[ModelInfo],
    known_model_ids: &HashSet<String>,
    config_default_model: Option<&str>,
) -> Result<Option<AgentHandoff>, DispatchError> {
    info!(session_id, "dispatching reclassification");
    let result = reclassify(request, llm_client, router_model, available_models)?;

    event_sink.emit_prompt_classified(session_id, &result.new_output);

    if !result.mode_changed {
        debug!("reclassification: mode unchanged");
        return Ok(None);
    }

    let loaded_mode = mode_loader.load_mode(&result.new_output.mode)?;
    let model = resolve_model(
        result.new_output.model.as_deref(),
        &loaded_mode,
        known_model_ids,
        config_default_model,
    );

    info!(
        new_mode = %result.new_output.mode,
        new_model = %model,
        "reclassification: mode changed, new handoff prepared"
    );
    Ok(Some(AgentHandoff {
        mode_name: result.new_output.mode,
        system_prompt: loaded_mode.system_prompt,
        model,
        context: request.updated_context.clone(),
        allowed_tools: loaded_mode.allowed_tools,
        prompt: request.prompt.clone(),
        confidence: result.new_output.confidence,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt_router::classify::ClassificationError;
    use crate::prompt_router::user_override::ModeOverriddenEvent;
    use std::collections::HashSet;
    use std::sync::Mutex;

    struct MockLlmClient {
        response: String,
    }

    impl LlmClient for MockLlmClient {
        fn complete(&self, _prompt: &str, _model: &str) -> Result<String, ClassificationError> {
            Ok(self.response.clone())
        }
    }

    struct MockModeLoader;

    impl ModeLoader for MockModeLoader {
        fn load_mode(&self, mode_name: &str) -> Result<LoadedMode, DispatchError> {
            match mode_name {
                "Plan" => Ok(LoadedMode {
                    name: "Plan".to_string(),
                    system_prompt: "You are a planning assistant.".to_string(),
                    default_model: Some("fallback-model".to_string()),
                    allowed_tools: vec!["read".to_string(), "search".to_string()],
                }),
                "Implement" => Ok(LoadedMode {
                    name: "Implement".to_string(),
                    system_prompt: "You are a code generation assistant.".to_string(),
                    default_model: Some("code-model".to_string()),
                    allowed_tools: vec![
                        "read".to_string(),
                        "write".to_string(),
                        "exec".to_string(),
                    ],
                }),
                _ => Err(DispatchError::ModeNotFound(mode_name.to_string())),
            }
        }
    }

    struct RecordingEventSink {
        classified: Mutex<Vec<RouterOutput>>,
        overridden: Mutex<Vec<ModeOverriddenEvent>>,
    }

    impl RecordingEventSink {
        fn new() -> Self {
            Self {
                classified: Mutex::new(Vec::new()),
                overridden: Mutex::new(Vec::new()),
            }
        }
    }

    impl RouterEventSink for RecordingEventSink {
        fn emit_prompt_classified(&self, _session_id: &str, output: &RouterOutput) {
            self.classified.lock().unwrap().push(output.clone());
        }

        fn emit_mode_overridden(&self, _session_id: &str, event: &ModeOverriddenEvent) {
            self.overridden.lock().unwrap().push(event.clone());
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
                languages: vec!["Rust".to_string()],
                frameworks: vec!["Tauri".to_string()],
                file_structure_hints: vec![],
            },
        }
    }

    #[test]
    fn dispatch_classification_path() {
        let input = sample_input();
        let llm = MockLlmClient {
            response: r#"{"mode":"Plan","model":"anthropic/claude-sonnet-4-6","confidence":0.8}"#
                .to_string(),
        };
        let sink = RecordingEventSink::new();
        let known: HashSet<String> = ["anthropic/claude-sonnet-4-6".to_string()]
            .into_iter()
            .collect();

        let handoff = dispatch(
            &input,
            None,
            &llm,
            "router-model",
            &MockModeLoader,
            &sink,
            "sess-1",
            &[],
            &known,
            None,
        )
        .unwrap();

        assert_eq!(handoff.mode_name, "Plan");
        assert_eq!(handoff.system_prompt, "You are a planning assistant.");
        assert_eq!(handoff.model, "anthropic/claude-sonnet-4-6");
        assert_eq!(handoff.prompt, "Implement auth middleware");
        assert_eq!(handoff.confidence, 0.8);
        assert_eq!(handoff.allowed_tools, vec!["read", "search"]);
        assert_eq!(sink.classified.lock().unwrap().len(), 1);
        assert_eq!(sink.overridden.lock().unwrap().len(), 0);
    }

    #[test]
    fn dispatch_override_path() {
        let input = sample_input();
        let llm = MockLlmClient {
            response: String::new(), // won't be called
        };
        let sink = RecordingEventSink::new();

        let handoff = dispatch(
            &input,
            Some("Implement"),
            &llm,
            "router-model",
            &MockModeLoader,
            &sink,
            "sess-1",
            &[],
            &HashSet::new(),
            None,
        )
        .unwrap();

        assert_eq!(handoff.mode_name, "Implement");
        assert_eq!(
            handoff.system_prompt,
            "You are a code generation assistant."
        );
        assert_eq!(handoff.model, "code-model");
        assert_eq!(handoff.confidence, 1.0);
        assert_eq!(sink.classified.lock().unwrap().len(), 0);
        assert_eq!(sink.overridden.lock().unwrap().len(), 1);
        assert_eq!(sink.overridden.lock().unwrap()[0].from_mode, "none");
        assert_eq!(sink.overridden.lock().unwrap()[0].to_mode, "Implement");
    }

    #[test]
    fn dispatch_override_unknown_mode_returns_error() {
        let input = sample_input();
        let llm = MockLlmClient {
            response: String::new(),
        };
        let sink = RecordingEventSink::new();

        let err = dispatch(
            &input,
            Some("Unknown"),
            &llm,
            "router-model",
            &MockModeLoader,
            &sink,
            "sess-1",
            &[],
            &HashSet::new(),
            None,
        )
        .unwrap_err();

        assert!(matches!(err, DispatchError::Override(_)));
    }

    #[test]
    fn dispatch_mode_not_found_returns_error() {
        let mut input = sample_input();
        // Add a mode that the LLM can classify to but ModeLoader doesn't know
        input.available_modes.push(ModeInfo {
            name: "Debug".to_string(),
            description: "Debugging mode".to_string(),
        });

        let llm = MockLlmClient {
            response: r#"{"mode":"Debug","model":"claude","confidence":0.7}"#.to_string(),
        };
        let sink = RecordingEventSink::new();

        let err = dispatch(
            &input,
            None,
            &llm,
            "router-model",
            &MockModeLoader,
            &sink,
            "sess-1",
            &[],
            &HashSet::new(),
            None,
        )
        .unwrap_err();

        assert!(matches!(err, DispatchError::ModeNotFound(ref name) if name == "Debug"));
    }

    #[test]
    fn dispatch_uses_mode_default_when_classifier_returns_empty_model() {
        let input = sample_input();
        let llm = MockLlmClient {
            response: r#"{"mode":"Plan","model":"","confidence":0.9}"#.to_string(),
        };
        let sink = RecordingEventSink::new();
        let known: HashSet<String> = ["fallback-model".to_string()].into_iter().collect();

        let handoff = dispatch(
            &input,
            None,
            &llm,
            "router-model",
            &MockModeLoader,
            &sink,
            "sess-1",
            &[],
            &known,
            None,
        )
        .unwrap();

        // Empty model from classifier → falls back to mode's default_model
        assert_eq!(handoff.model, "fallback-model");
    }

    #[test]
    fn dispatch_uses_mode_default_when_catalog_empty() {
        let input = sample_input();
        let llm = MockLlmClient {
            response: r#"{"mode":"Plan","model":"some-model","confidence":0.9}"#.to_string(),
        };
        let sink = RecordingEventSink::new();

        let handoff = dispatch(
            &input,
            None,
            &llm,
            "router-model",
            &MockModeLoader,
            &sink,
            "sess-1",
            &[],
            &HashSet::new(),
            None,
        )
        .unwrap();

        // Empty catalog → router model not validated, falls to mode default
        assert_eq!(handoff.model, "fallback-model");
    }

    #[test]
    fn resolve_model_validates_router_selection() {
        let loaded = LoadedMode {
            name: "Plan".into(),
            system_prompt: String::new(),
            default_model: Some("anthropic/claude-opus-4-6".into()),
            allowed_tools: vec![],
        };
        let known: HashSet<String> = ["anthropic/claude-sonnet-4-6".to_string()]
            .into_iter()
            .collect();

        // Known router model passes through
        assert_eq!(
            resolve_model(Some("anthropic/claude-sonnet-4-6"), &loaded, &known, None),
            "anthropic/claude-sonnet-4-6"
        );

        // Unknown router model → falls to mode default (not FALLBACK_MODEL)
        assert_eq!(
            resolve_model(Some("hallucinated/model-9000"), &loaded, &known, None),
            "anthropic/claude-opus-4-6"
        );
    }

    #[test]
    fn resolve_model_passes_through_mode_default() {
        let loaded = LoadedMode {
            name: "Plan".into(),
            system_prompt: String::new(),
            default_model: Some("anthropic/claude-opus-4-6".into()),
            allowed_tools: vec![],
        };

        // Empty catalog → mode default passes through as-is
        assert_eq!(
            resolve_model(Some("any/model"), &loaded, &HashSet::new(), None),
            "anthropic/claude-opus-4-6"
        );
    }

    #[test]
    fn resolve_model_returns_fallback_when_nothing_configured() {
        let loaded = LoadedMode {
            name: "Plan".into(),
            system_prompt: String::new(),
            default_model: None,
            allowed_tools: vec![],
        };
        let known: HashSet<String> = ["anthropic/claude-sonnet-4-6".to_string()]
            .into_iter()
            .collect();

        // No router model, no mode default → FALLBACK_MODEL
        assert_eq!(resolve_model(None, &loaded, &known, None), FALLBACK_MODEL);
    }

    #[test]
    fn dispatch_reclassification_returns_none_when_mode_unchanged() {
        let request = ReclassificationRequest {
            trigger: crate::prompt_router::reclassify::ReclassificationTrigger::UserRequested,
            current_mode: "Plan".to_string(),
            current_model: "claude-sonnet".to_string(),
            prompt: "Continue planning".to_string(),
            updated_context: CompactedContext {
                messages_summary: "Planning session".to_string(),
                learnings: vec![],
                preserved_facts: vec![],
                token_count: 50,
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
            project_context: ProjectContext::default(),
        };

        let llm = MockLlmClient {
            response: r#"{"mode":"Plan","model":"claude-sonnet","confidence":0.9}"#.to_string(),
        };
        let sink = RecordingEventSink::new();

        let result = dispatch_reclassification(
            &request,
            &llm,
            "router-model",
            &MockModeLoader,
            &sink,
            "sess-1",
            &[],
            &HashSet::new(),
            None,
        )
        .unwrap();

        assert!(result.is_none());
        assert_eq!(sink.classified.lock().unwrap().len(), 1);
    }

    #[test]
    fn dispatch_reclassification_returns_handoff_when_mode_changed() {
        let request = ReclassificationRequest {
            trigger: crate::prompt_router::reclassify::ReclassificationTrigger::UserRequested,
            current_mode: "Plan".to_string(),
            current_model: "claude-sonnet".to_string(),
            prompt: "Let's start coding".to_string(),
            updated_context: CompactedContext {
                messages_summary: "Planning done, ready to implement".to_string(),
                learnings: vec![],
                preserved_facts: vec![],
                token_count: 80,
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
            project_context: ProjectContext::default(),
        };

        let llm = MockLlmClient {
            response: r#"{"mode":"Implement","model":"code-model","confidence":0.85}"#.to_string(),
        };
        let sink = RecordingEventSink::new();
        let known: HashSet<String> = ["code-model".to_string()].into_iter().collect();

        let result = dispatch_reclassification(
            &request,
            &llm,
            "router-model",
            &MockModeLoader,
            &sink,
            "sess-1",
            &[],
            &known,
            None,
        )
        .unwrap();

        let handoff = result.expect("should return Some when mode changed");
        assert_eq!(handoff.mode_name, "Implement");
        assert_eq!(
            handoff.system_prompt,
            "You are a code generation assistant."
        );
        assert_eq!(handoff.model, "code-model");
        assert_eq!(handoff.confidence, 0.85);
        assert_eq!(handoff.prompt, "Let's start coding");
    }

    #[test]
    fn from_classification_error() {
        let err: DispatchError = ClassificationError {
            message: "test".to_string(),
        }
        .into();
        assert!(matches!(err, DispatchError::Classification(_)));
        assert!(err.to_string().contains("Classification failed: "));
    }

    #[test]
    fn from_override_error() {
        let err: DispatchError = OverrideError {
            message: "test".to_string(),
        }
        .into();
        assert!(matches!(err, DispatchError::Override(_)));
        assert!(err.to_string().contains("Override failed: "));
    }
}
