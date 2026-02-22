use std::fmt::Write;

use super::triggers::{EventSummary, TriggerReason};

pub const UX_AGENT_SYSTEM_PROMPT: &str = r#"You are Kernel's UX Agent, a background assistant that analyzes usage patterns and makes recommendations to improve the user's workflow.

Your responsibilities:
1. Detect patterns in user behavior from aggregated event data
2. Propose mode creation/modification when recurring patterns emerge
3. Recommend model switches based on performance data
4. Surface warnings about concerning patterns
5. Suggest configuration adjustments

Rules:
- Be conservative: only recommend when evidence is strong
- Be specific: reference the data patterns you observed
- Be actionable: each recommendation should be immediately applicable
- Never auto-apply: all recommendations require user approval
- Do not re-suggest patterns the user has already dismissed

Your output MUST be valid JSON matching this schema:
{
  "recommendations": [
    {
      "trigger_pattern": "description of what you observed",
      "recommendation": "human-readable summary",
      "action": {
        "type": "ModelChange | PromptEdit | ModeCreate | ModeEdit | ConfigChange",
        ...variant fields
      }
    }
  ]
}

Action variant fields:
- ModelChange: { "type": "ModelChange", "role": "...", "from_model": "...", "to_model": "..." }
- PromptEdit: { "type": "PromptEdit", "mode_name": "...", "old_fragment": "...", "new_fragment": "..." }
- ModeCreate: { "type": "ModeCreate", "name": "...", "description": "...", "system_prompt": "...", "default_model": null|"...", "allowed_tools": ["..."] }
- ModeEdit: { "type": "ModeEdit", "mode_name": "...", "changes": { "description": null|"...", "system_prompt": null|"...", "default_model": null|"...", "allowed_tools": null|["..."] } }
- ConfigChange: { "type": "ConfigChange", "key": "...", "old_value": "...", "new_value": "..." }

If there is nothing to recommend, return: { "recommendations": [] }
"#;

pub fn build_user_message(
    triggers: &[TriggerReason],
    summary: &EventSummary,
    config_snapshot: &str,
    modes_snapshot: &str,
    dismissed_patterns: &[String],
) -> String {
    let mut msg = String::with_capacity(1024);

    // Triggers
    msg.push_str("## Triggers fired\n");
    for trigger in triggers {
        let _ = writeln!(msg, "- {}", format_trigger(trigger));
    }
    msg.push('\n');

    // Event summary
    msg.push_str("## Event summary\n");
    let _ = writeln!(msg, "- Rejections: {}", summary.rejection_count);
    let _ = writeln!(msg, "- New session: {}", summary.is_new_session);
    let _ = writeln!(msg, "- Recent cost (USD): {:.4}", summary.recent_cost_usd);
    let _ = writeln!(
        msg,
        "- Baseline cost (USD): {:.4}",
        summary.baseline_cost_usd
    );
    if !summary.tool_failures.is_empty() {
        msg.push_str("- Tool failures:\n");
        for (tool, count) in &summary.tool_failures {
            let _ = writeln!(msg, "  - {tool}: {count} failures");
        }
    }
    if !summary.mode_overrides.is_empty() {
        msg.push_str("- Mode overrides:\n");
        for (override_type, count) in &summary.mode_overrides {
            let _ = writeln!(msg, "  - {override_type}: {count} overrides");
        }
    }
    msg.push('\n');

    // Config
    msg.push_str("## Current configuration\n");
    msg.push_str(config_snapshot);
    msg.push_str("\n\n");

    // Modes
    msg.push_str("## Available modes\n");
    msg.push_str(modes_snapshot);
    msg.push_str("\n\n");

    // Dismissed patterns
    if !dismissed_patterns.is_empty() {
        msg.push_str("## Previously dismissed patterns (do NOT re-suggest)\n");
        for pattern in dismissed_patterns {
            let _ = writeln!(msg, "- {pattern}");
        }
        msg.push('\n');
    }

    msg
}

fn format_trigger(trigger: &TriggerReason) -> String {
    match trigger {
        TriggerReason::RejectionsAccumulated { count } => {
            format!("{count} rejections accumulated")
        }
        TriggerReason::NewSession => "New session started".to_string(),
        TriggerReason::CostSpike {
            current_rate_usd,
            baseline_rate_usd,
        } => format!(
            "Cost spike: ${current_rate_usd:.4}/session vs ${baseline_rate_usd:.4} baseline"
        ),
        TriggerReason::FailurePattern {
            tool,
            failure_count,
        } => format!("Tool '{tool}' failed {failure_count} times"),
        TriggerReason::OverridePattern {
            override_type,
            count,
        } => format!("Override pattern: '{override_type}' overridden {count} times"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_is_non_empty() {
        assert!(!UX_AGENT_SYSTEM_PROMPT.is_empty());
        assert!(UX_AGENT_SYSTEM_PROMPT.contains("recommendations"));
    }

    #[test]
    fn build_user_message_includes_triggers() {
        let triggers = vec![
            TriggerReason::RejectionsAccumulated { count: 5 },
            TriggerReason::NewSession,
        ];
        let summary = EventSummary {
            rejection_count: 5,
            is_new_session: true,
            has_new_events: true,
            ..Default::default()
        };
        let msg = build_user_message(&triggers, &summary, "{}", "[]", &[]);
        assert!(msg.contains("5 rejections accumulated"));
        assert!(msg.contains("New session started"));
        assert!(msg.contains("Rejections: 5"));
    }

    #[test]
    fn build_user_message_includes_dismissed_patterns() {
        let msg = build_user_message(
            &[TriggerReason::NewSession],
            &EventSummary {
                is_new_session: true,
                has_new_events: true,
                ..Default::default()
            },
            "{}",
            "[]",
            &["3+ diff rejections on schema edits".to_string()],
        );
        assert!(msg.contains("Previously dismissed"));
        assert!(msg.contains("3+ diff rejections on schema edits"));
    }

    #[test]
    fn build_user_message_omits_dismissed_when_empty() {
        let msg = build_user_message(
            &[TriggerReason::NewSession],
            &EventSummary {
                is_new_session: true,
                has_new_events: true,
                ..Default::default()
            },
            "{}",
            "[]",
            &[],
        );
        assert!(!msg.contains("dismissed"));
    }

    #[test]
    fn build_user_message_includes_tool_failures_and_overrides() {
        let summary = EventSummary {
            tool_failures: vec![("bash".to_string(), 4)],
            mode_overrides: vec![("strict".to_string(), 3)],
            has_new_events: true,
            ..Default::default()
        };
        let msg = build_user_message(
            &[TriggerReason::FailurePattern {
                tool: "bash".to_string(),
                failure_count: 4,
            }],
            &summary,
            "{}",
            "[]",
            &[],
        );
        assert!(msg.contains("bash: 4 failures"));
        assert!(msg.contains("strict: 3 overrides"));
    }
}
