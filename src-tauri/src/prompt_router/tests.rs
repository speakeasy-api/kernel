use super::classify::{self, build_classification_prompt, classify, LlmClient};
use super::dispatch::{
    dispatch, dispatch_reclassification, DispatchError, LoadedMode, ModeLoader, RouterEventSink,
};
use super::reclassify::{self, ReclassificationTrigger};
use super::types::*;
use super::user_override::{self, apply_override, ModeOverriddenEvent};
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Mock implementations
// ---------------------------------------------------------------------------

struct MockLlmClient {
    response: String,
}

impl MockLlmClient {
    fn new(response: &str) -> Self {
        Self {
            response: response.to_string(),
        }
    }

    fn failing(error_msg: &str) -> Self {
        Self {
            response: format!("ERROR: {}", error_msg),
        }
    }
}

impl LlmClient for MockLlmClient {
    fn complete(
        &self,
        _prompt: &str,
        _model: &str,
    ) -> Result<String, classify::ClassificationError> {
        if self.response.starts_with("ERROR: ") {
            Err(classify::ClassificationError {
                message: self.response.clone(),
            })
        } else {
            Ok(self.response.clone())
        }
    }
}

struct MockEventSink {
    classified_events: Mutex<Vec<RouterOutput>>,
    override_events: Mutex<Vec<ModeOverriddenEvent>>,
}

impl MockEventSink {
    fn new() -> Self {
        Self {
            classified_events: Mutex::new(Vec::new()),
            override_events: Mutex::new(Vec::new()),
        }
    }
}

impl RouterEventSink for MockEventSink {
    fn emit_prompt_classified(&self, _session_id: &str, output: &RouterOutput) {
        self.classified_events.lock().unwrap().push(output.clone());
    }
    fn emit_mode_overridden(&self, _session_id: &str, event: &ModeOverriddenEvent) {
        self.override_events.lock().unwrap().push(event.clone());
    }
}

struct MockModeLoader;

impl ModeLoader for MockModeLoader {
    fn load_mode(&self, mode_name: &str) -> Result<LoadedMode, DispatchError> {
        match mode_name {
            "Plan" | "Implement" | "Review" | "Debug" | "Research" | "General" => {
                Ok(LoadedMode {
                    name: mode_name.to_string(),
                    system_prompt: format!("You are in {} mode.", mode_name),
                    default_model: Some("default-model".to_string()),
                    allowed_tools: vec!["fs_read".to_string()],
                })
            }
            _ => Err(DispatchError::ModeNotFound(mode_name.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_modes() -> Vec<ModeInfo> {
    vec![
        ModeInfo {
            name: "Plan".into(),
            description: "Planning mode".into(),
        },
        ModeInfo {
            name: "Implement".into(),
            description: "Implementation mode".into(),
        },
        ModeInfo {
            name: "Review".into(),
            description: "Review mode".into(),
        },
        ModeInfo {
            name: "Debug".into(),
            description: "Debug mode".into(),
        },
        ModeInfo {
            name: "Research".into(),
            description: "Research mode".into(),
        },
        ModeInfo {
            name: "General".into(),
            description: "General mode".into(),
        },
    ]
}

fn test_context() -> CompactedContext {
    CompactedContext {
        messages_summary: "User has been discussing project architecture.".into(),
        learnings: vec![],
        preserved_facts: vec!["src-tauri/src/lib.rs".into()],
        token_count: 100,
    }
}

fn test_project_context() -> ProjectContext {
    ProjectContext {
        languages: vec!["Rust".into()],
        frameworks: vec!["Tauri".into()],
        file_structure_hints: vec!["src-tauri/src/".into()],
    }
}

fn test_input() -> RouterInput {
    RouterInput {
        source: PromptSource::User,
        prompt: "Refactor the event system to use channels".into(),
        available_modes: test_modes(),
        conversation_history: test_context(),
        project_context: test_project_context(),
    }
}

// ---------------------------------------------------------------------------
// Types tests
// ---------------------------------------------------------------------------

#[test]
fn test_prompt_source_serialization() {
    let user = PromptSource::User;
    let json = serde_json::to_string(&user).unwrap();
    let round_tripped: PromptSource = serde_json::from_str(&json).unwrap();
    assert_eq!(round_tripped, PromptSource::User);

    let async_root = PromptSource::AsyncTaskRoot;
    let json = serde_json::to_string(&async_root).unwrap();
    let round_tripped: PromptSource = serde_json::from_str(&json).unwrap();
    assert_eq!(round_tripped, PromptSource::AsyncTaskRoot);
}

#[test]
fn test_project_context_default() {
    let ctx = ProjectContext::default();
    assert!(ctx.languages.is_empty());
    assert!(ctx.frameworks.is_empty());
    assert!(ctx.file_structure_hints.is_empty());
}

#[test]
fn test_router_output_serialization() {
    let output = RouterOutput {
        mode: "Plan".into(),
        model: "claude-sonnet".into(),
        confidence: 0.85,
    };
    let json = serde_json::to_string(&output).unwrap();
    let round_tripped: RouterOutput = serde_json::from_str(&json).unwrap();
    assert_eq!(round_tripped.mode, "Plan");
    assert_eq!(round_tripped.model, "claude-sonnet");
    assert_eq!(round_tripped.confidence, 0.85);
}

// ---------------------------------------------------------------------------
// Classification tests
// ---------------------------------------------------------------------------

#[test]
fn test_classify_clean_json() {
    let input = test_input();
    let llm = MockLlmClient::new(
        r#"{"mode":"Implement","model":"claude-3","confidence":0.9}"#,
    );
    let result = classify(&input, &llm, "router-model").unwrap();
    assert_eq!(result.mode, "Implement");
    assert_eq!(result.model, "claude-3");
    assert_eq!(result.confidence, 0.9);
}

#[test]
fn test_classify_json_with_surrounding_text() {
    let input = test_input();
    let llm = MockLlmClient::new(
        r#"Here is the result: {"mode":"Plan","model":"claude-3","confidence":0.85} hope that helps!"#,
    );
    let result = classify(&input, &llm, "router-model").unwrap();
    assert_eq!(result.mode, "Plan");
    assert_eq!(result.model, "claude-3");
    assert_eq!(result.confidence, 0.85);
}

#[test]
fn test_classify_invalid_json() {
    let input = test_input();
    let llm = MockLlmClient::new("I don't know");
    let result = classify(&input, &llm, "router-model");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.message.contains("failed to parse"));
}

#[test]
fn test_classify_invalid_mode() {
    let input = test_input();
    let llm = MockLlmClient::new(
        r#"{"mode":"Nonexistent","model":"claude-3","confidence":0.9}"#,
    );
    let result = classify(&input, &llm, "router-model");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.message.contains("unknown mode 'Nonexistent'"));
}

#[test]
fn test_classify_confidence_clamping() {
    let input = test_input();
    let llm = MockLlmClient::new(
        r#"{"mode":"Plan","model":"claude-3","confidence":1.5}"#,
    );
    let result = classify(&input, &llm, "router-model").unwrap();
    assert_eq!(result.confidence, 1.0);
}

#[test]
fn test_classify_negative_confidence() {
    let input = test_input();
    let llm = MockLlmClient::new(
        r#"{"mode":"Plan","model":"claude-3","confidence":-0.5}"#,
    );
    let result = classify(&input, &llm, "router-model").unwrap();
    assert_eq!(result.confidence, 0.0);
}

#[test]
fn test_build_classification_prompt_contains_modes() {
    let input = test_input();
    let prompt = build_classification_prompt(&input);
    for mode in &input.available_modes {
        assert!(
            prompt.contains(&mode.name),
            "prompt should contain mode name '{}'",
            mode.name
        );
        assert!(
            prompt.contains(&mode.description),
            "prompt should contain mode description '{}'",
            mode.description
        );
    }
}

#[test]
fn test_build_classification_prompt_contains_user_prompt() {
    let input = test_input();
    let prompt = build_classification_prompt(&input);
    assert!(prompt.contains("Refactor the event system to use channels"));
}

#[test]
fn test_build_classification_prompt_contains_project_context() {
    let input = test_input();
    let prompt = build_classification_prompt(&input);
    assert!(prompt.contains("Rust"));
    assert!(prompt.contains("Tauri"));
}

// ---------------------------------------------------------------------------
// Override tests
// ---------------------------------------------------------------------------

#[test]
fn test_apply_override_valid_mode() {
    let original = RouterOutput {
        mode: "Implement".into(),
        model: "claude-sonnet".into(),
        confidence: 0.7,
    };
    let (output, _event) =
        apply_override(&original, "Plan", None, &test_modes()).unwrap();
    assert_eq!(output.mode, "Plan");
    assert_eq!(output.confidence, 1.0);
}

#[test]
fn test_apply_override_invalid_mode() {
    let original = RouterOutput {
        mode: "Implement".into(),
        model: "claude-sonnet".into(),
        confidence: 0.7,
    };
    let result = apply_override(&original, "Nonexistent", None, &test_modes());
    assert!(result.is_err());
}

#[test]
fn test_apply_override_preserves_model_when_none() {
    let original = RouterOutput {
        mode: "Implement".into(),
        model: "claude-sonnet".into(),
        confidence: 0.7,
    };
    let (output, _) = apply_override(&original, "Plan", None, &test_modes()).unwrap();
    assert_eq!(output.model, "claude-sonnet");
}

#[test]
fn test_apply_override_uses_override_model() {
    let original = RouterOutput {
        mode: "Implement".into(),
        model: "claude-sonnet".into(),
        confidence: 0.7,
    };
    let (output, _) =
        apply_override(&original, "Plan", Some("gpt-4"), &test_modes()).unwrap();
    assert_eq!(output.model, "gpt-4");
}

#[test]
fn test_apply_override_event_data() {
    let original = RouterOutput {
        mode: "Implement".into(),
        model: "claude-sonnet".into(),
        confidence: 0.7,
    };
    let (_, event) = apply_override(&original, "Plan", None, &test_modes()).unwrap();
    assert_eq!(event.from_mode, "Implement");
    assert_eq!(event.to_mode, "Plan");
}

// ---------------------------------------------------------------------------
// Reclassification tests
// ---------------------------------------------------------------------------

fn test_reclass_request(current_mode: &str) -> reclassify::ReclassificationRequest {
    reclassify::ReclassificationRequest {
        trigger: ReclassificationTrigger::UserRequested,
        current_mode: current_mode.to_string(),
        current_model: "claude-sonnet".to_string(),
        prompt: "Actually, let me plan this first".to_string(),
        updated_context: test_context(),
        available_modes: test_modes(),
        project_context: test_project_context(),
    }
}

#[test]
fn test_reclassify_mode_changed() {
    let request = test_reclass_request("Implement");
    let llm = MockLlmClient::new(
        r#"{"mode":"Plan","model":"claude-sonnet","confidence":0.85}"#,
    );
    let result = reclassify::reclassify(&request, &llm, "router-model").unwrap();
    assert!(result.mode_changed);
    assert_eq!(result.new_output.mode, "Plan");
    assert_eq!(result.previous_mode, "Implement");
}

#[test]
fn test_reclassify_same_mode() {
    let request = test_reclass_request("Implement");
    let llm = MockLlmClient::new(
        r#"{"mode":"Implement","model":"claude-sonnet","confidence":0.9}"#,
    );
    let result = reclassify::reclassify(&request, &llm, "router-model").unwrap();
    assert!(!result.mode_changed);
    assert_eq!(result.new_output.mode, "Implement");
}

#[test]
fn test_reclassify_preserves_trigger() {
    let mut request = test_reclass_request("Plan");
    request.trigger = ReclassificationTrigger::OrchestratorDetected {
        reason: "Shifted to coding".into(),
    };
    let llm = MockLlmClient::new(
        r#"{"mode":"Implement","model":"claude-sonnet","confidence":0.8}"#,
    );
    let result = reclassify::reclassify(&request, &llm, "router-model").unwrap();
    assert_eq!(
        result.trigger,
        ReclassificationTrigger::OrchestratorDetected {
            reason: "Shifted to coding".into(),
        }
    );
}

#[test]
fn test_reclassify_llm_error() {
    let request = test_reclass_request("Plan");
    let llm = MockLlmClient::failing("LLM unavailable");
    let result = reclassify::reclassify(&request, &llm, "router-model");
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Dispatch tests
// ---------------------------------------------------------------------------

#[test]
fn test_dispatch_classification_path() {
    let input = test_input();
    let llm = MockLlmClient::new(
        r#"{"mode":"Plan","model":"claude-sonnet","confidence":0.8}"#,
    );
    let sink = MockEventSink::new();

    let handoff = dispatch(
        &input,
        None,
        &llm,
        "router-model",
        &MockModeLoader,
        &sink,
        "sess-1",
    )
    .unwrap();

    assert_eq!(handoff.mode_name, "Plan");
    assert_eq!(handoff.system_prompt, "You are in Plan mode.");
    assert_eq!(handoff.model, "claude-sonnet");
    assert_eq!(handoff.confidence, 0.8);
    assert_eq!(sink.classified_events.lock().unwrap().len(), 1);
    assert_eq!(sink.override_events.lock().unwrap().len(), 0);
}

#[test]
fn test_dispatch_override_path() {
    let input = test_input();
    let llm = MockLlmClient::new(""); // won't be called
    let sink = MockEventSink::new();

    let handoff = dispatch(
        &input,
        Some("Implement"),
        &llm,
        "router-model",
        &MockModeLoader,
        &sink,
        "sess-1",
    )
    .unwrap();

    assert_eq!(handoff.mode_name, "Implement");
    assert_eq!(handoff.confidence, 1.0);
    assert_eq!(sink.classified_events.lock().unwrap().len(), 0);
    assert_eq!(sink.override_events.lock().unwrap().len(), 1);
}

#[test]
fn test_dispatch_mode_not_found() {
    let mut input = test_input();
    input.available_modes.push(ModeInfo {
        name: "Exotic".into(),
        description: "Exotic mode".into(),
    });
    let llm = MockLlmClient::new(
        r#"{"mode":"Exotic","model":"claude","confidence":0.7}"#,
    );
    let sink = MockEventSink::new();

    let err = dispatch(
        &input,
        None,
        &llm,
        "router-model",
        &MockModeLoader,
        &sink,
        "sess-1",
    )
    .unwrap_err();

    assert!(matches!(err, DispatchError::ModeNotFound(ref name) if name == "Exotic"));
}

#[test]
fn test_dispatch_classification_error() {
    let input = test_input();
    let llm = MockLlmClient::failing("service down");
    let sink = MockEventSink::new();

    let err = dispatch(
        &input,
        None,
        &llm,
        "router-model",
        &MockModeLoader,
        &sink,
        "sess-1",
    )
    .unwrap_err();

    assert!(matches!(err, DispatchError::Classification(_)));
}

#[test]
fn test_dispatch_reclassification_mode_changed() {
    let request = test_reclass_request("Plan");
    let llm = MockLlmClient::new(
        r#"{"mode":"Implement","model":"code-model","confidence":0.85}"#,
    );
    let sink = MockEventSink::new();

    let result = dispatch_reclassification(
        &request,
        &llm,
        "router-model",
        &MockModeLoader,
        &sink,
        "sess-1",
    )
    .unwrap();

    let handoff = result.expect("should return Some when mode changed");
    assert_eq!(handoff.mode_name, "Implement");
    assert_eq!(handoff.system_prompt, "You are in Implement mode.");
    assert_eq!(handoff.model, "code-model");
    assert_eq!(handoff.confidence, 0.85);
}

#[test]
fn test_dispatch_reclassification_no_change() {
    let request = test_reclass_request("Plan");
    let llm = MockLlmClient::new(
        r#"{"mode":"Plan","model":"claude-sonnet","confidence":0.9}"#,
    );
    let sink = MockEventSink::new();

    let result = dispatch_reclassification(
        &request,
        &llm,
        "router-model",
        &MockModeLoader,
        &sink,
        "sess-1",
    )
    .unwrap();

    assert!(result.is_none());
}

#[test]
fn test_dispatch_handoff_fields() {
    let input = test_input();
    let llm = MockLlmClient::new(
        r#"{"mode":"Review","model":"review-model","confidence":0.92}"#,
    );
    let sink = MockEventSink::new();

    let handoff = dispatch(
        &input,
        None,
        &llm,
        "router-model",
        &MockModeLoader,
        &sink,
        "sess-1",
    )
    .unwrap();

    assert_eq!(handoff.mode_name, "Review");
    assert_eq!(handoff.system_prompt, "You are in Review mode.");
    assert_eq!(handoff.model, "review-model");
    assert_eq!(handoff.allowed_tools, vec!["fs_read"]);
    assert_eq!(handoff.prompt, "Refactor the event system to use channels");
    assert_eq!(handoff.confidence, 0.92);
    assert_eq!(
        handoff.context.messages_summary,
        "User has been discussing project architecture."
    );
}
