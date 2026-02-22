use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub tool_name: String,
    pub args_summary: String,
}

/// Threshold for loop detection: number of similar tool calls before flagging.
pub const LOOP_THRESHOLD: usize = 3;

/// Similarity threshold for argument comparison (0.0 to 1.0).
/// Arguments are considered "similar" if similarity >= this value.
pub const ARG_SIMILARITY_THRESHOLD: f64 = 0.8;

const LOOP_DETECTION_WINDOW: usize = 10;
const MAX_HISTORY_PER_AGENT: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopAction {
    /// No loop detected, continue normally
    Continue,
    /// Loop detected — first occurrence. Emit event and suggest retry with modified prompt.
    RetryWithModifiedPrompt {
        agent_id: Uuid,
        repeated_tool: String,
        count: u32,
        suggestion: String,
    },
    /// Retry failed (loop detected again after retry). Spawn an unstuck agent.
    SpawnUnstuckAgent {
        agent_id: Uuid,
        repeated_tool: String,
        count: u32,
        loop_context: String,
    },
    /// Unstuck agent also failed. Escalate to user.
    EscalateToUser {
        agent_id: Uuid,
        repeated_tool: String,
        count: u32,
        message: String,
    },
}

#[derive(Debug, Default)]
pub struct LoopDetector {
    /// Per-agent tool call history
    history: HashMap<Uuid, Vec<ToolCallRecord>>,
}

impl LoopDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a tool call for an agent. Returns the action to take.
    ///
    /// The `retry_count` parameter tracks how many times we've already
    /// attempted to break this particular loop:
    ///   0 = first detection -> RetryWithModifiedPrompt
    ///   1 = retry failed -> SpawnUnstuckAgent
    ///   2+ = unstuck failed -> EscalateToUser
    pub fn record_call(
        &mut self,
        agent_id: Uuid,
        call: ToolCallRecord,
        retry_count: u32,
    ) -> LoopAction {
        let calls = self.history.entry(agent_id).or_default();
        calls.push(call);

        if calls.len() > MAX_HISTORY_PER_AGENT {
            let overflow = calls.len() - MAX_HISTORY_PER_AGENT;
            calls.drain(0..overflow);
        }

        let Some((tool, count)) = self.detect_loop(&agent_id) else {
            return LoopAction::Continue;
        };

        let history = self.history(&agent_id);
        match retry_count {
            0 => LoopAction::RetryWithModifiedPrompt {
                agent_id,
                repeated_tool: tool.clone(),
                count,
                suggestion: build_retry_suggestion(&tool, history),
            },
            1 => LoopAction::SpawnUnstuckAgent {
                agent_id,
                repeated_tool: tool.clone(),
                count,
                loop_context: build_loop_context(&agent_id, &tool, history),
            },
            _ => LoopAction::EscalateToUser {
                agent_id,
                repeated_tool: tool.clone(),
                count,
                message: build_escalation_message(&tool, count),
            },
        }
    }

    /// Check if the recent tool calls for an agent contain a loop pattern.
    /// Returns Some((tool_name, count)) if a loop is detected.
    fn detect_loop(&self, agent_id: &Uuid) -> Option<(String, u32)> {
        let calls = self.history.get(agent_id)?;
        if calls.len() < LOOP_THRESHOLD {
            return None;
        }

        let window_start = calls.len().saturating_sub(LOOP_DETECTION_WINDOW);
        let recent_calls = &calls[window_start..];

        let mut grouped: HashMap<&str, Vec<&ToolCallRecord>> = HashMap::new();
        for call in recent_calls {
            grouped
                .entry(call.tool_name.as_str())
                .or_default()
                .push(call);
        }

        // Check more recent tools first for faster, deterministic detection.
        let mut ordered_tools = Vec::new();
        let mut seen = HashSet::new();
        for call in recent_calls.iter().rev() {
            let tool = call.tool_name.as_str();
            if seen.insert(tool) {
                ordered_tools.push(tool);
            }
        }

        for tool in ordered_tools {
            let Some(records) = grouped.get(tool) else {
                continue;
            };

            if records.len() < LOOP_THRESHOLD {
                continue;
            }

            if arguments_are_similar(records) {
                return Some((tool.to_string(), records.len() as u32));
            }
        }

        None
    }

    /// Clear history for an agent (call when agent completes or fails).
    pub fn clear(&mut self, agent_id: &Uuid) {
        self.history.remove(agent_id);
    }

    /// Get the full history for an agent (useful for building unstuck context).
    pub fn history(&self, agent_id: &Uuid) -> &[ToolCallRecord] {
        self.history.get(agent_id).map(Vec::as_slice).unwrap_or(&[])
    }
}

fn arguments_are_similar(records: &[&ToolCallRecord]) -> bool {
    for i in 0..records.len() {
        for j in (i + 1)..records.len() {
            if args_similar(&records[i].args_summary, &records[j].args_summary)
                < ARG_SIMILARITY_THRESHOLD
            {
                return false;
            }
        }
    }
    true
}

fn build_retry_suggestion(tool: &str, history: &[ToolCallRecord]) -> String {
    let tool_calls = history.iter().filter(|r| r.tool_name == tool).count();
    format!(
        "The agent is repeatedly calling '{}' with similar arguments ({} calls seen). \
         Suggest a different approach or tool to accomplish the same goal.",
        tool, tool_calls
    )
}

fn build_loop_context(agent_id: &Uuid, tool: &str, history: &[ToolCallRecord]) -> String {
    let recent: Vec<String> = history
        .iter()
        .filter(|r| r.tool_name == tool)
        .map(|r| format!("  {}({})", r.tool_name, r.args_summary))
        .collect();

    format!(
        "Agent {} is stuck in a loop calling '{}':\n{}\n\
         Please analyze why these calls are failing and suggest a different approach.",
        agent_id,
        tool,
        recent.join("\n")
    )
}

fn build_escalation_message(tool: &str, count: u32) -> String {
    format!(
        "Agent is stuck: called '{}' {} times with similar arguments. \
         Automated retry and unstuck agent both failed. Human intervention needed.",
        tool, count
    )
}

/// Compare two argument summaries for similarity.
/// Returns a value between 0.0 (completely different) and 1.0 (identical).
fn args_similar(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let a_tokens: HashSet<&str> = a.split_whitespace().collect();
    let b_tokens: HashSet<&str> = b.split_whitespace().collect();

    let intersection = a_tokens.intersection(&b_tokens).count();
    let union = a_tokens.union(&b_tokens).count();
    if union == 0 {
        return 0.0;
    }

    intersection as f64 / union as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(tool: &str, args: &str) -> ToolCallRecord {
        ToolCallRecord {
            tool_name: tool.to_string(),
            args_summary: args.to_string(),
        }
    }

    #[test]
    fn args_similar_identical_strings() {
        assert_eq!(
            args_similar("path src-tauri/src", "path src-tauri/src"),
            1.0
        );
    }

    #[test]
    fn args_similar_different_strings() {
        assert_eq!(args_similar("find cat", "write patch"), 0.0);
    }

    #[test]
    fn args_similar_empty_string_returns_zero() {
        assert_eq!(args_similar("", "read src"), 0.0);
        assert_eq!(args_similar("read src", ""), 0.0);
    }

    #[test]
    fn args_similar_partial_overlap_is_between_zero_and_one() {
        let similarity = args_similar("read src tauri agent", "read src tauri tests");
        assert!(similarity > 0.0);
        assert!(similarity < 1.0);
    }

    #[test]
    fn no_loop_returns_continue() {
        let agent_id = Uuid::new_v4();
        let mut detector = LoopDetector::new();

        assert_eq!(
            detector.record_call(agent_id, call("search", "foo bar"), 0),
            LoopAction::Continue
        );
        assert_eq!(
            detector.record_call(agent_id, call("search", "foo baz"), 0),
            LoopAction::Continue
        );
    }

    #[test]
    fn different_tools_do_not_trigger_loop() {
        let agent_id = Uuid::new_v4();
        let mut detector = LoopDetector::new();

        detector.record_call(agent_id, call("search", "foo bar"), 0);
        detector.record_call(agent_id, call("read_file", "src/main.rs"), 0);
        let action = detector.record_call(agent_id, call("git_status", "short"), 0);

        assert!(matches!(action, LoopAction::Continue));
    }

    #[test]
    fn first_detection_requests_retry() {
        let agent_id = Uuid::new_v4();
        let mut detector = LoopDetector::new();

        detector.record_call(agent_id, call("search", "foo bar"), 0);
        detector.record_call(agent_id, call("search", "foo bar"), 0);
        let action = detector.record_call(agent_id, call("search", "foo bar"), 0);

        match action {
            LoopAction::RetryWithModifiedPrompt {
                repeated_tool,
                count,
                ..
            } => {
                assert_eq!(repeated_tool, "search");
                assert_eq!(count, 3);
            }
            _ => panic!("expected RetryWithModifiedPrompt"),
        }
    }

    #[test]
    fn second_detection_spawns_unstuck_agent() {
        let agent_id = Uuid::new_v4();
        let mut detector = LoopDetector::new();

        detector.record_call(agent_id, call("search", "foo bar"), 0);
        detector.record_call(agent_id, call("search", "foo bar"), 0);
        let action = detector.record_call(agent_id, call("search", "foo bar"), 1);

        match action {
            LoopAction::SpawnUnstuckAgent {
                repeated_tool,
                count,
                ..
            } => {
                assert_eq!(repeated_tool, "search");
                assert_eq!(count, 3);
            }
            _ => panic!("expected SpawnUnstuckAgent"),
        }
    }

    #[test]
    fn repeated_failure_escalates_to_user() {
        let agent_id = Uuid::new_v4();
        let mut detector = LoopDetector::new();

        detector.record_call(agent_id, call("search", "foo bar"), 0);
        detector.record_call(agent_id, call("search", "foo bar"), 0);
        let action = detector.record_call(agent_id, call("search", "foo bar"), 2);

        match action {
            LoopAction::EscalateToUser {
                repeated_tool,
                count,
                ..
            } => {
                assert_eq!(repeated_tool, "search");
                assert_eq!(count, 3);
            }
            _ => panic!("expected EscalateToUser"),
        }
    }

    #[test]
    fn similar_but_not_identical_args_can_trigger_threshold_detection() {
        let agent_id = Uuid::new_v4();
        let mut detector = LoopDetector::new();

        detector.record_call(agent_id, call("search", "find src tauri agent"), 0);
        detector.record_call(agent_id, call("search", "find src tauri agent extra"), 0);
        let action = detector.record_call(agent_id, call("search", "find src tauri agent"), 0);

        match action {
            LoopAction::RetryWithModifiedPrompt {
                repeated_tool,
                count,
                ..
            } => {
                assert_eq!(repeated_tool, "search");
                assert_eq!(count, 3);
            }
            _ => panic!("expected RetryWithModifiedPrompt"),
        }
    }

    #[test]
    fn clear_removes_agent_history() {
        let agent_id = Uuid::new_v4();
        let mut detector = LoopDetector::new();

        detector.record_call(agent_id, call("search", "foo bar"), 0);
        assert_eq!(detector.history(&agent_id).len(), 1);

        detector.clear(&agent_id);
        assert!(detector.history(&agent_id).is_empty());
    }

    #[test]
    fn history_is_bounded() {
        let agent_id = Uuid::new_v4();
        let mut detector = LoopDetector::new();

        for i in 0..60 {
            detector.record_call(agent_id, call("search", &format!("q{}", i)), 0);
        }

        assert_eq!(detector.history(&agent_id).len(), 50);
        assert_eq!(detector.history(&agent_id)[0].args_summary, "q10");
    }

    #[test]
    fn detect_loop_requires_similar_arguments_for_same_tool() {
        let agent_id = Uuid::new_v4();
        let mut detector = LoopDetector::new();

        detector.record_call(agent_id, call("search", "apple banana"), 0);
        detector.record_call(agent_id, call("search", "apple cherry"), 0);
        let action = detector.record_call(agent_id, call("search", "apple banana"), 0);

        assert!(matches!(action, LoopAction::Continue));
    }

    #[test]
    fn dissimilar_arguments_do_not_trigger_loop() {
        let agent_id = Uuid::new_v4();
        let mut detector = LoopDetector::new();

        detector.record_call(agent_id, call("write_file", "path src/alpha.rs"), 0);
        detector.record_call(agent_id, call("write_file", "path docs/readme.md"), 0);
        let action = detector.record_call(agent_id, call("write_file", "path Cargo.toml"), 0);

        assert!(matches!(action, LoopAction::Continue));
    }
}
