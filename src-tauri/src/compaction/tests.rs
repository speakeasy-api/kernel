use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;

use super::*;

fn make_message(role: &str, content: &str) -> Message {
    Message {
        role: role.to_string(),
        content: content.to_string(),
    }
}

fn make_messages(pairs: &[(&str, &str)]) -> Vec<Message> {
    pairs
        .iter()
        .map(|(role, content)| make_message(role, content))
        .collect()
}

struct MockLlmState {
    response: Mutex<Result<String, String>>,
    calls: AtomicUsize,
    prompts: Mutex<Vec<(String, String)>>,
}

#[derive(Clone)]
struct MockLlmClient {
    state: Arc<MockLlmState>,
}

impl MockLlmClient {
    fn new(response: &str) -> Self {
        Self {
            state: Arc::new(MockLlmState {
                response: Mutex::new(Ok(response.to_string())),
                calls: AtomicUsize::new(0),
                prompts: Mutex::new(Vec::new()),
            }),
        }
    }

    fn with_error(error: &str) -> Self {
        Self {
            state: Arc::new(MockLlmState {
                response: Mutex::new(Err(error.to_string())),
                calls: AtomicUsize::new(0),
                prompts: Mutex::new(Vec::new()),
            }),
        }
    }

    fn failing() -> Self {
        Self::with_error("mock failure")
    }

    fn call_count(&self) -> usize {
        self.state.calls.load(Ordering::SeqCst)
    }

    fn last_user_prompt(&self) -> Option<String> {
        self.state
            .prompts
            .lock()
            .expect("mock prompts mutex poisoned")
            .last()
            .map(|(_, user)| user.clone())
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String, CompactionError> {
        self.state.calls.fetch_add(1, Ordering::SeqCst);
        self.state
            .prompts
            .lock()
            .expect("mock prompts mutex poisoned")
            .push((system_prompt.to_string(), user_prompt.to_string()));

        match &*self
            .state
            .response
            .lock()
            .expect("mock response mutex poisoned")
        {
            Ok(response) => Ok(response.clone()),
            Err(error) => Err(CompactionError::LlmFailed(error.clone())),
        }
    }
}

// budget.rs tests

#[test]
fn test_budget_creation_valid() {
    let budget = ContextBudget::new(100_000, 5_000, 2_500, 0.6, 0.4).expect("valid budget");

    assert_eq!(budget.max_tokens, 100_000);
    assert_eq!(budget.reserved_system, 5_000);
    assert_eq!(budget.reserved_response, 2_500);
    assert_eq!(budget.compaction_trigger, 0.6);
    assert_eq!(budget.target_after_compaction, 0.4);
}

#[test]
fn test_budget_trigger_below_target() {
    let err = ContextBudget::new(100_000, 5_000, 2_500, 0.4, 0.4).expect_err("must fail");
    assert!(matches!(err, BudgetError::TriggerBelowTarget { .. }));
}

#[test]
fn test_budget_invalid_percentages() {
    let err = ContextBudget::new(100_000, 5_000, 2_500, 1.1, 0.4).expect_err("must fail");
    assert!(matches!(err, BudgetError::InvalidPercentage { .. }));
}

#[test]
fn test_budget_reserved_exceeds_max() {
    let err = ContextBudget::new(100, 60, 40, 0.7, 0.4).expect_err("must fail");
    assert!(matches!(err, BudgetError::ReservedExceedsMax { .. }));
}

#[test]
fn test_available_tokens() {
    let budget = ContextBudget::new(10_000, 1_000, 500, 0.8, 0.5).expect("valid budget");
    assert_eq!(budget.available_tokens(), 8_500);
}

#[test]
fn test_needs_deep_compaction_true() {
    let budget = ContextBudget::new(10_000, 500, 500, 0.6, 0.4).expect("valid budget");
    assert!(budget.needs_deep_compaction(6_001));
}

#[test]
fn test_needs_deep_compaction_false() {
    let budget = ContextBudget::new(10_000, 500, 500, 0.6, 0.4).expect("valid budget");
    assert!(!budget.needs_deep_compaction(5_999));
}

#[test]
fn test_needs_deep_compaction_exact_boundary() {
    let budget = ContextBudget::new(10_000, 500, 500, 0.6, 0.4).expect("valid budget");
    assert!(budget.needs_deep_compaction(6_000));
}

#[test]
fn test_tokens_to_reclaim() {
    let budget = ContextBudget::new(10_000, 500, 500, 0.6, 0.4).expect("valid budget");
    assert_eq!(budget.tokens_to_reclaim(8_000), 4_000);
}

#[test]
fn test_tokens_to_reclaim_when_not_needed() {
    let budget = ContextBudget::new(10_000, 500, 500, 0.6, 0.4).expect("valid budget");
    assert_eq!(budget.tokens_to_reclaim(5_000), 0);
}

#[test]
fn test_estimate_tokens_empty() {
    assert_eq!(estimate_tokens(""), 0);
}

#[test]
fn test_estimate_tokens_short() {
    assert_eq!(estimate_tokens("abc"), 1);
    assert_eq!(estimate_tokens("hello"), 2);
}

#[test]
fn test_estimate_tokens_long() {
    assert_eq!(estimate_tokens(&"x".repeat(400)), 100);
    assert_eq!(estimate_tokens(&"x".repeat(401)), 101);
}

#[test]
fn test_estimate_message_tokens() {
    let messages = make_messages(&[("user", "hello world"), ("assistant", "hi")]);
    assert_eq!(estimate_message_tokens(&messages), 12);
}

// structural.rs tests

#[test]
fn test_strip_thinking_tags_basic() {
    let messages = make_messages(&[("assistant", "keep <thinking>remove</thinking> this")]);
    let output = StructuralFilter::apply(&messages);
    assert_eq!(output[0].content, "keep  this");
}

#[test]
fn test_strip_thinking_tags_multiple() {
    let messages = make_messages(&[(
        "assistant",
        "a <thinking>x</thinking> b <thinking>y</thinking> c",
    )]);
    let output = StructuralFilter::apply(&messages);
    assert_eq!(output[0].content, "a  b  c");
}

#[test]
fn test_strip_thinking_tags_nested() {
    let messages = make_messages(&[(
        "assistant",
        "start <thinking>outer <thinking>inner</thinking> end</thinking> done",
    )]);
    let output = StructuralFilter::apply(&messages);
    assert_eq!(output[0].content, "start  done");
}

#[test]
fn test_strip_thinking_tags_no_match() {
    let messages = make_messages(&[("assistant", "no hidden content")]);
    let output = StructuralFilter::apply(&messages);
    assert_eq!(output[0].content, "no hidden content");
}

#[test]
fn test_strip_thinking_tags_antml_variant() {
    let messages = make_messages(&[("assistant", "a<thinking type=\"plan\">hidden</thinking>b")]);
    let output = StructuralFilter::apply(&messages);
    assert_eq!(output[0].content, "ab");
}

#[test]
fn test_collapse_tool_outputs_long() {
    let long = format!("{}{}", "a".repeat(1_900), "b".repeat(300));
    let messages = make_messages(&[("tool", &long)]);
    let output = StructuralFilter::apply(&messages);

    assert_eq!(output.len(), 1);
    assert!(output[0].content.starts_with(&"a".repeat(500)));
    assert!(output[0].content.contains("[truncated, 2200 chars total]"));
    assert!(output[0].content.ends_with(&"b".repeat(200)));
}

#[test]
fn test_collapse_tool_outputs_short() {
    let content = "x".repeat(1_999);
    let messages = make_messages(&[("tool", &content)]);
    let output = StructuralFilter::apply(&messages);

    assert_eq!(output.len(), 1);
    assert_eq!(output[0].content, content);
}

#[test]
fn test_collapse_tool_outputs_non_tool() {
    let content = "x".repeat(5_000);
    let messages = make_messages(&[("assistant", &content)]);
    let output = StructuralFilter::apply(&messages);

    assert_eq!(output.len(), 1);
    assert_eq!(output[0].content, content);
}

#[test]
fn test_deduplicate_reads_consecutive() {
    let messages = make_messages(&[
        ("tool", "File: /tmp/a.rs\none"),
        ("tool", "Contents of /tmp/a.rs:\ntwo"),
        ("tool", "File: /tmp/a.rs\nthree"),
    ]);
    let output = StructuralFilter::apply(&messages);

    assert_eq!(
        output[0].content,
        "[duplicate read of /tmp/a.rs — see later message]"
    );
    assert_eq!(
        output[1].content,
        "[duplicate read of /tmp/a.rs — see later message]"
    );
    assert_eq!(output[2].content, "File: /tmp/a.rs\nthree");
}

#[test]
fn test_deduplicate_reads_no_dupes() {
    let messages = make_messages(&[
        ("tool", "File: /tmp/a.rs\nfirst"),
        ("tool", "File: /tmp/b.rs\nsecond"),
    ]);
    let output = StructuralFilter::apply(&messages);

    assert_eq!(output[0].content, "File: /tmp/a.rs\nfirst");
    assert_eq!(output[1].content, "File: /tmp/b.rs\nsecond");
}

#[test]
fn test_deduplicate_reads_outside_window() {
    let mut messages = vec![make_message("tool", "File: /tmp/a.rs\nstart")];
    for i in 0..10 {
        messages.push(make_message("tool", &format!("File: /tmp/other{i}.rs\nx")));
    }
    messages.push(make_message("tool", "File: /tmp/a.rs\nlate"));

    let output = StructuralFilter::apply(&messages);
    assert_eq!(output[0].content, "File: /tmp/a.rs\nstart");
}

#[test]
fn test_strip_redundant_whitespace() {
    let messages = make_messages(&[("assistant", "line with trailing   \n\n\n\nsecond line   ")]);
    let output = StructuralFilter::apply(&messages);

    assert_eq!(output[0].content, "line with trailing\n\nsecond line");
}

#[test]
fn test_strip_redundant_whitespace_preserves_indent() {
    let messages = make_messages(&[("assistant", "    keep   this  aligned   ")]);
    let output = StructuralFilter::apply(&messages);

    assert_eq!(output[0].content, "    keep this  aligned");
}

#[test]
fn test_apply_skips_system_prompt() {
    let messages = make_messages(&[("system", "<thinking>do not touch</thinking>   ")]);
    let output = StructuralFilter::apply(&messages);

    assert_eq!(output[0].content, "<thinking>do not touch</thinking>   ");
}

#[test]
fn test_apply_full_pipeline() {
    let messages = make_messages(&[
        ("system", "System prompt"),
        ("tool", &format!("File: /tmp/a.rs\n{}", "x".repeat(2_100))),
        ("tool", "File: /tmp/a.rs\nlatest"),
        (
            "assistant",
            "<thinking>draft</thinking>text with    noise   \n\n\n",
        ),
    ]);

    let output = StructuralFilter::apply(&messages);

    assert_eq!(output.len(), 4);
    assert_eq!(output[0].content, "System prompt");
    assert_eq!(
        output[1].content,
        "[duplicate read of /tmp/a.rs — see later message]"
    );
    assert_eq!(output[2].content, "File: /tmp/a.rs\nlatest");
    assert_eq!(output[3].content, "text with noise\n\n");
}

// preservation.rs tests

#[test]
fn test_extract_file_paths() {
    let rules = PreservationRules {
        preserved_patterns: vec![PreservedPattern::FilePath],
    };
    let messages = make_messages(&[(
        "assistant",
        "Use /absolute/path.rs and ./relative/path.txt, skip https://example.com/foo.rs",
    )]);

    let facts = rules.extract_preserved_facts(&messages);

    assert!(facts.contains(&"/absolute/path.rs".to_string()));
    assert!(facts.contains(&"./relative/path.txt".to_string()));
    assert!(!facts.iter().any(|f| f.contains("example.com")));
}

#[test]
fn test_extract_function_signatures_rust() {
    let rules = PreservationRules {
        preserved_patterns: vec![PreservedPattern::FunctionSignature],
    };
    let messages = make_messages(&[(
        "assistant",
        "pub fn run() -> bool { true }\nasync fn fetch() {}",
    )]);

    let facts = rules.extract_preserved_facts(&messages);

    assert!(facts.iter().any(|f| f.starts_with("pub fn run()")));
    assert!(facts.iter().any(|f| f.starts_with("async fn fetch()")));
}

#[test]
fn test_extract_function_signatures_python() {
    let rules = PreservationRules {
        preserved_patterns: vec![PreservedPattern::FunctionSignature],
    };
    let messages = make_messages(&[("assistant", "def hello(name):\nasync def fetch():")]);

    let facts = rules.extract_preserved_facts(&messages);

    assert!(facts.iter().any(|f| f.starts_with("def hello(")));
    assert!(facts.iter().any(|f| f.starts_with("async def fetch(")));
}

#[test]
fn test_extract_function_signatures_typescript() {
    let rules = PreservationRules {
        preserved_patterns: vec![PreservedPattern::FunctionSignature],
    };
    let messages = make_messages(&[(
        "assistant",
        "function greet(name: string) {}\nexport async function load() {}",
    )]);

    let facts = rules.extract_preserved_facts(&messages);

    assert!(facts.iter().any(|f| f.starts_with("function greet(")));
    assert!(facts
        .iter()
        .any(|f| f.starts_with("export async function load(")));
}

#[test]
fn test_extract_error_messages() {
    let rules = PreservationRules {
        preserved_patterns: vec![PreservedPattern::ErrorMessage],
    };
    let messages = make_messages(&[(
        "assistant",
        "error[E0308]: mismatched types\nError: invalid config\nthread 'main' panicked",
    )]);

    let facts = rules.extract_preserved_facts(&messages);

    assert!(facts.iter().any(|f| f.contains("error[E0308]")));
    assert!(facts.iter().any(|f| f.contains("Error: invalid config")));
    assert!(facts.iter().any(|f| f.contains("thread 'main' panicked")));
}

#[test]
fn test_extract_decision_records() {
    let rules = PreservationRules {
        preserved_patterns: vec![PreservedPattern::DecisionRecord],
    };
    let messages = make_messages(&[(
        "assistant",
        "Decision: use crate A\nbecause it is stable\n\nDecided: keep API minimal",
    )]);

    let facts = rules.extract_preserved_facts(&messages);

    assert!(facts.iter().any(|f| f.starts_with("Decision: use crate A")));
    assert!(facts
        .iter()
        .any(|f| f.starts_with("Decided: keep API minimal")));
}

#[test]
fn test_extract_task_state() {
    let rules = PreservationRules {
        preserved_patterns: vec![PreservedPattern::TaskState],
    };
    let messages = make_messages(&[(
        "assistant",
        "Task: write tests\nWorking on: semantic coverage\nNext: run cargo test",
    )]);

    let facts = rules.extract_preserved_facts(&messages);

    assert!(facts.iter().any(|f| f.starts_with("Task: write tests")));
    assert!(facts
        .iter()
        .any(|f| f.starts_with("Working on: semantic coverage")));
    assert!(facts.iter().any(|f| f.starts_with("Next: run cargo test")));
}

#[test]
fn test_extract_preserved_facts_deduplication() {
    let rules = PreservationRules::default_rules();
    let messages = make_messages(&[
        ("user", "Look at /foo/bar.rs"),
        ("assistant", "I also checked /foo/bar.rs"),
    ]);

    let facts = rules.extract_preserved_facts(&messages);
    let count = facts.iter().filter(|f| *f == "/foo/bar.rs").count();

    assert_eq!(count, 1);
}

#[test]
fn test_protected_message_indices() {
    let rules = PreservationRules::default_rules();
    let messages = make_messages(&[
        ("system", "System"),
        ("user", "User"),
        ("assistant", "plain assistant"),
        ("assistant", "Decision: choose approach B"),
    ]);

    let indices = rules.protected_message_indices(&messages);

    assert!(indices.contains(&0));
    assert!(indices.contains(&1));
    assert!(!indices.contains(&2));
    assert!(indices.contains(&3));
}

#[test]
fn test_default_rules() {
    let rules = PreservationRules::default_rules();
    assert_eq!(rules.preserved_patterns.len(), 5);
    assert!(rules
        .preserved_patterns
        .iter()
        .any(|p| matches!(p, PreservedPattern::FilePath)));
    assert!(rules
        .preserved_patterns
        .iter()
        .any(|p| matches!(p, PreservedPattern::FunctionSignature)));
    assert!(rules
        .preserved_patterns
        .iter()
        .any(|p| matches!(p, PreservedPattern::ErrorMessage)));
    assert!(rules
        .preserved_patterns
        .iter()
        .any(|p| matches!(p, PreservedPattern::DecisionRecord)));
    assert!(rules
        .preserved_patterns
        .iter()
        .any(|p| matches!(p, PreservedPattern::TaskState)));
}

// semantic.rs tests

#[tokio::test]
async fn test_compact_parses_valid_json() {
    let response = r#"{"messages":[{"role":"assistant","content":"done"}],"learnings":[],"preserved_facts":[]}"#;
    let client = MockLlmClient::new(response);
    let budget = ContextBudget::new(100_000, 5_000, 5_000, 0.6, 0.4).expect("valid budget");
    let compactor = SemanticCompactor::new(client, budget);
    let rules = PreservationRules::default_rules();

    let result = compactor
        .compact(&make_messages(&[("user", "hello")]), &rules)
        .await
        .expect("compaction should succeed");

    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].content, "done");
}

#[tokio::test]
async fn test_compact_parses_json_in_code_block() {
    let response = "```json\n{\"messages\":[{\"role\":\"assistant\",\"content\":\"done\"}],\"learnings\":[\"l1\"],\"preserved_facts\":[\"/tmp/a.rs\"]}\n```";
    let client = MockLlmClient::new(response);
    let budget = ContextBudget::new(100_000, 5_000, 5_000, 0.6, 0.4).expect("valid budget");
    let compactor = SemanticCompactor::new(client, budget);
    let rules = PreservationRules::default_rules();

    let result = compactor
        .compact(&make_messages(&[("user", "hello")]), &rules)
        .await
        .expect("compaction should succeed");

    assert_eq!(result.messages[0].content, "done");
    assert_eq!(result.learnings, vec!["l1".to_string()]);
    assert_eq!(result.preserved_facts, vec!["/tmp/a.rs".to_string()]);
}

#[tokio::test]
async fn test_compact_llm_failure() {
    let client = MockLlmClient::failing();
    let budget = ContextBudget::new(100_000, 5_000, 5_000, 0.6, 0.4).expect("valid budget");
    let compactor = SemanticCompactor::new(client, budget);
    let rules = PreservationRules::default_rules();

    let result = compactor
        .compact(&make_messages(&[("user", "hello")]), &rules)
        .await;

    assert!(matches!(result, Err(CompactionError::LlmFailed(_))));
}

#[tokio::test]
async fn test_compact_parse_failure() {
    let client = MockLlmClient::new("this is not json");
    let budget = ContextBudget::new(100_000, 5_000, 5_000, 0.6, 0.4).expect("valid budget");
    let compactor = SemanticCompactor::new(client, budget);
    let rules = PreservationRules::default_rules();

    let result = compactor
        .compact(&make_messages(&[("user", "hello")]), &rules)
        .await;

    assert!(matches!(result, Err(CompactionError::ParseFailed(_))));
}

#[tokio::test]
async fn test_compaction_prompt_includes_preserved_items() {
    let response = r#"{"messages":[{"role":"assistant","content":"done"}],"learnings":[],"preserved_facts":[]}"#;
    let client = MockLlmClient::new(response);
    let client_probe = client.clone();

    let budget = ContextBudget::new(100_000, 5_000, 5_000, 0.6, 0.4).expect("valid budget");
    let compactor = SemanticCompactor::new(client, budget);

    let messages = make_messages(&[(
        "assistant",
        "Decision: keep /tmp/app.rs\npub fn run() {}\nTask: write tests",
    )]);

    compactor
        .compact(&messages, &PreservationRules::default_rules())
        .await
        .expect("compaction should succeed");

    let prompt = client_probe
        .last_user_prompt()
        .expect("user prompt should be captured");

    assert!(prompt.contains("Items that MUST be preserved"));
    assert!(prompt.contains("/tmp/app.rs"));
    assert!(prompt.contains("pub fn run() {}"));
    assert!(prompt.contains("Task: write tests"));
}

// pipeline.rs tests

#[tokio::test]
async fn test_pipeline_light_only() {
    let client = MockLlmClient::new(r#"{"messages":[],"learnings":[],"preserved_facts":[]}"#);
    let client_probe = client.clone();
    let budget = ContextBudget::new(100_000, 5_000, 5_000, 0.9, 0.5).expect("valid budget");
    let pipeline =
        CompactionPipeline::new(budget, PreservationRules::default_rules(), client, true);

    let messages = make_messages(&[("assistant", "<thinking>hidden</thinking>visible")]);
    let result = pipeline
        .compact("system", &messages)
        .await
        .expect("pipeline ok");

    assert_eq!(result.messages[0].content, "visible");
    assert_eq!(client_probe.call_count(), 0);
}

#[tokio::test]
async fn test_pipeline_light_and_deep() {
    let response = r#"{"messages":[{"role":"assistant","content":"deep result"}],"learnings":[],"preserved_facts":[]}"#;
    let client = MockLlmClient::new(response);
    let client_probe = client.clone();

    let budget = ContextBudget::new(40, 0, 0, 0.6, 0.4).expect("valid budget");
    let pipeline =
        CompactionPipeline::new(budget, PreservationRules::default_rules(), client, true);

    let messages = make_messages(&[
        ("assistant", "<thinking>hidden</thinking>"),
        ("user", &"x".repeat(200)),
    ]);
    let result = pipeline
        .compact("sys", &messages)
        .await
        .expect("pipeline ok");

    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].content, "deep result");
    assert_eq!(client_probe.call_count(), 1);

    let prompt = client_probe
        .last_user_prompt()
        .expect("user prompt should exist");
    assert!(!prompt.contains("<thinking>hidden</thinking>"));
}

#[tokio::test]
async fn test_pipeline_deep_failure_fallback() {
    let client = MockLlmClient::with_error("network down");
    let client_probe = client.clone();
    let budget = ContextBudget::new(40, 0, 0, 0.6, 0.4).expect("valid budget");
    let pipeline =
        CompactionPipeline::new(budget, PreservationRules::default_rules(), client, true);

    let messages = make_messages(&[
        ("assistant", "<thinking>secret</thinking>final"),
        ("user", &"x".repeat(200)),
    ]);
    let result = pipeline
        .compact("system", &messages)
        .await
        .expect("pipeline ok");

    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].content, "final");
    assert_eq!(client_probe.call_count(), 1);
}

#[tokio::test]
async fn test_pipeline_no_compaction() {
    let client = MockLlmClient::new(r#"{"messages":[],"learnings":[],"preserved_facts":[]}"#);
    let client_probe = client.clone();

    let budget = ContextBudget::new(100_000, 5_000, 5_000, 0.95, 0.5).expect("valid budget");
    let pipeline =
        CompactionPipeline::new(budget, PreservationRules::default_rules(), client, false);

    let messages = make_messages(&[("assistant", "<thinking>keep</thinking>content")]);
    let result = pipeline
        .compact("system", &messages)
        .await
        .expect("pipeline ok");

    assert_eq!(result.messages.len(), messages.len());
    assert_eq!(result.messages[0].role, messages[0].role);
    assert_eq!(result.messages[0].content, messages[0].content);
    assert_eq!(client_probe.call_count(), 0);
}

#[tokio::test]
async fn test_pipeline_output_token_count() {
    let client = MockLlmClient::new(r#"{"messages":[],"learnings":[],"preserved_facts":[]}"#);
    let budget = ContextBudget::new(100_000, 5_000, 5_000, 0.95, 0.5).expect("valid budget");
    let pipeline =
        CompactionPipeline::new(budget, PreservationRules::default_rules(), client, true);

    let messages = make_messages(&[("assistant", "hello"), ("user", "world")]);
    let result = pipeline
        .compact("system prompt", &messages)
        .await
        .expect("pipeline ok");

    let expected = estimate_message_tokens(&result.messages) + estimate_tokens("system prompt");
    assert_eq!(result.token_count, expected);
}

#[test]
fn test_pipeline_from_config() {
    let client = MockLlmClient::new(r#"{"messages":[],"learnings":[],"preserved_facts":[]}"#);

    let pipeline = CompactionPipeline::from_config(100_000, 5_000, 5_000, true, 0.6, 0.4, client);
    assert!(pipeline.is_ok());
}

#[test]
fn test_pipeline_from_config_invalid() {
    let client = MockLlmClient::new(r#"{"messages":[],"learnings":[],"preserved_facts":[]}"#);

    let pipeline = CompactionPipeline::from_config(100, 60, 50, true, 0.6, 0.4, client);
    assert!(matches!(
        pipeline,
        Err(BudgetError::ReservedExceedsMax { .. })
    ));
}

#[tokio::test]
async fn test_extract_learnings_tool_errors() {
    let client = MockLlmClient::new(r#"{"messages":[],"learnings":[],"preserved_facts":[]}"#);
    let budget = ContextBudget::new(100_000, 5_000, 5_000, 0.95, 0.5).expect("valid budget");
    let pipeline =
        CompactionPipeline::new(budget, PreservationRules::default_rules(), client, false);

    let messages = make_messages(&[("tool", "Error: permission denied reading /tmp/a.rs")]);
    let result = pipeline
        .compact("sys", &messages)
        .await
        .expect("pipeline ok");

    assert_eq!(result.learnings.len(), 1);
    assert!(result.learnings[0].starts_with("Tool call failed: "));
}

#[tokio::test]
async fn test_extract_learnings_learning_prefix() {
    let client = MockLlmClient::new(r#"{"messages":[],"learnings":[],"preserved_facts":[]}"#);
    let budget = ContextBudget::new(100_000, 5_000, 5_000, 0.95, 0.5).expect("valid budget");
    let pipeline =
        CompactionPipeline::new(budget, PreservationRules::default_rules(), client, false);

    let messages = make_messages(&[(
        "assistant",
        "Some text\nLEARNING: avoid stale cache\nmore text",
    )]);
    let result = pipeline
        .compact("sys", &messages)
        .await
        .expect("pipeline ok");

    assert_eq!(
        result.learnings,
        vec!["LEARNING: avoid stale cache".to_string()]
    );
}

#[tokio::test]
async fn test_extract_learnings_dedup() {
    let client = MockLlmClient::new(r#"{"messages":[],"learnings":[],"preserved_facts":[]}"#);
    let budget = ContextBudget::new(100_000, 5_000, 5_000, 0.95, 0.5).expect("valid budget");
    let pipeline =
        CompactionPipeline::new(budget, PreservationRules::default_rules(), client, false);

    let messages = make_messages(&[
        ("tool", "error: file not found /tmp/x"),
        ("tool", "Error:   file   not found   /tmp/x"),
        ("assistant", "LEARNING: keep retries bounded"),
        ("assistant", "LEARNING: keep retries bounded"),
    ]);
    let result = pipeline
        .compact("sys", &messages)
        .await
        .expect("pipeline ok");

    assert_eq!(result.learnings.len(), 2);
    assert!(result
        .learnings
        .iter()
        .any(|item| item.starts_with("Tool call failed: ")));
    assert!(result
        .learnings
        .contains(&"LEARNING: keep retries bounded".to_string()));
}
