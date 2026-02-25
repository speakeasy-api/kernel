use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use crate::ux_agent::triggers::{EventSummary, TriggerReason};
use crate::ux_agent::types::{ModeChanges, RecommendationAction};

/// Additional context for decision generation beyond EventSummary.
#[derive(Debug, Clone)]
pub struct DecisionContext {
    /// Currently available mode names.
    pub available_modes: Vec<String>,
    /// Current model assignments by role: (role, model_name).
    pub current_models: Vec<(String, String)>,
    /// Previously dismissed recommendation trigger patterns (to avoid re-suggesting).
    pub dismissed_patterns: Vec<String>,
    /// Recently applied recommendation summaries (to avoid duplicates).
    pub recent_applied: Vec<String>,
}

/// A candidate decision the model should consider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionCandidate {
    pub trigger: String,
    pub suggested_action_type: String,
    pub evidence: String,
    pub context: String,
}

/// Generate decision candidates from triggers and context.
///
/// These candidates are included in the model prompt to guide its output.
#[instrument(skip(_summary, context), fields(trigger_count = triggers.len()))]
pub fn generate_candidates(
    triggers: &[TriggerReason],
    _summary: &EventSummary,
    context: &DecisionContext,
) -> Vec<DecisionCandidate> {
    info!(
        trigger_count = triggers.len(),
        available_modes = ?context.available_modes,
        dismissed_patterns = context.dismissed_patterns.len(),
        "making UX decision from triggers"
    );
    let mut candidates = Vec::new();

    for trigger in triggers {
        match trigger {
            TriggerReason::RejectionsAccumulated { count } => {
                candidates.push(DecisionCandidate {
                    trigger: format!("rejections_accumulated:{}", count),
                    suggested_action_type: "mode_edit".to_string(),
                    evidence: format!("{} rejections since last run", count),
                    context: "Consider adjusting mode system prompts to address rejection patterns"
                        .to_string(),
                });
            }
            TriggerReason::CostSpike {
                current_rate_usd,
                baseline_rate_usd,
            } => {
                candidates.push(DecisionCandidate {
                    trigger: format!(
                        "cost_spike:{:.1}x",
                        current_rate_usd / baseline_rate_usd
                    ),
                    suggested_action_type: "model_change".to_string(),
                    evidence: format!(
                        "Cost rate ${:.4}/run vs baseline ${:.4}/run ({:.1}x increase)",
                        current_rate_usd,
                        baseline_rate_usd,
                        current_rate_usd / baseline_rate_usd
                    ),
                    context: format!("Current models: {:?}", context.current_models),
                });
            }
            TriggerReason::FailurePattern {
                tool,
                failure_count,
            } => {
                candidates.push(DecisionCandidate {
                    trigger: format!("failure_pattern:{}:{}", tool, failure_count),
                    suggested_action_type: "warning".to_string(),
                    evidence: format!("Tool '{}' has failed {} times", tool, failure_count),
                    context:
                        "Consider whether this tool should be restricted or if usage guidance is needed"
                            .to_string(),
                });
            }
            TriggerReason::OverridePattern {
                override_type,
                count,
            } => {
                candidates.push(DecisionCandidate {
                    trigger: format!("override_pattern:{}:{}", override_type, count),
                    suggested_action_type: "mode_create".to_string(),
                    evidence: format!("'{}' override applied {} times", override_type, count),
                    context: format!(
                        "Available modes: {:?}. Consider creating a new mode or editing an existing one.",
                        context.available_modes
                    ),
                });
            }
            TriggerReason::NewSession => {
                candidates.push(DecisionCandidate {
                    trigger: "new_session".to_string(),
                    suggested_action_type: "review".to_string(),
                    evidence: "New session started, reviewing accumulated patterns".to_string(),
                    context: "Look for overall trends since last run".to_string(),
                });
            }
        }
    }

    // Filter out candidates matching dismissed patterns
    let pre_filter_count = candidates.len();
    candidates.retain(|c| {
        let dominated = context
            .dismissed_patterns
            .iter()
            .any(|p| c.trigger.contains(p));
        if dominated {
            warn!(trigger = %c.trigger, "filtering out candidate matching dismissed pattern");
        }
        !dominated
    });

    if pre_filter_count != candidates.len() {
        debug!(
            before = pre_filter_count,
            after = candidates.len(),
            "candidates filtered by dismissed patterns"
        );
    }

    info!(candidate_count = candidates.len(), "UX decision made");
    candidates
}

#[instrument(skip(system_prompt))]
pub fn build_mode_create_action(
    name: &str,
    description: &str,
    system_prompt: &str,
    default_model: Option<&str>,
    allowed_tools: &[&str],
) -> RecommendationAction {
    info!(name, description, default_model, tools = ?allowed_tools, "building mode_create action");
    RecommendationAction::ModeCreate {
        name: name.to_string(),
        description: description.to_string(),
        system_prompt: system_prompt.to_string(),
        default_model: default_model.map(|s| s.to_string()),
        allowed_tools: allowed_tools.iter().map(|s| s.to_string()).collect(),
    }
}

#[instrument]
pub fn build_mode_edit_action(mode_name: &str, changes: ModeChanges) -> RecommendationAction {
    info!(mode_name, ?changes, "building mode_edit action");
    RecommendationAction::ModeEdit {
        mode_name: mode_name.to_string(),
        changes,
    }
}

#[instrument]
pub fn build_model_change_action(
    role: &str,
    from_model: &str,
    to_model: &str,
) -> RecommendationAction {
    info!(role, from_model, to_model, "building model_change action");
    RecommendationAction::ModelChange {
        role: role.to_string(),
        from_model: from_model.to_string(),
        to_model: to_model.to_string(),
    }
}

#[instrument]
pub fn build_config_change_action(
    key: &str,
    old_value: &str,
    new_value: &str,
) -> RecommendationAction {
    info!(key, old_value, new_value, "building config_change action");
    RecommendationAction::ConfigChange {
        key: key.to_string(),
        old_value: old_value.to_string(),
        new_value: new_value.to_string(),
    }
}

#[instrument(skip(old_fragment, new_fragment))]
pub fn build_prompt_edit_action(
    mode_name: &str,
    old_fragment: &str,
    new_fragment: &str,
) -> RecommendationAction {
    info!(mode_name, "building prompt_edit action");
    RecommendationAction::PromptEdit {
        mode_name: mode_name.to_string(),
        old_fragment: old_fragment.to_string(),
        new_fragment: new_fragment.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_context() -> DecisionContext {
        DecisionContext {
            available_modes: vec!["default".to_string(), "strict".to_string()],
            current_models: vec![
                ("planner".to_string(), "gpt-4".to_string()),
                ("coder".to_string(), "gpt-3.5".to_string()),
            ],
            dismissed_patterns: Vec::new(),
            recent_applied: Vec::new(),
        }
    }

    fn empty_summary() -> EventSummary {
        EventSummary::default()
    }

    #[test]
    fn generates_candidate_for_rejections() {
        let triggers = vec![TriggerReason::RejectionsAccumulated { count: 5 }];
        let candidates = generate_candidates(&triggers, &empty_summary(), &test_context());

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].trigger, "rejections_accumulated:5");
        assert_eq!(candidates[0].suggested_action_type, "mode_edit");
        assert!(candidates[0].evidence.contains("5 rejections"));
    }

    #[test]
    fn generates_candidate_for_cost_spike() {
        let triggers = vec![TriggerReason::CostSpike {
            current_rate_usd: 3.0,
            baseline_rate_usd: 1.0,
        }];
        let candidates = generate_candidates(&triggers, &empty_summary(), &test_context());

        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].trigger.starts_with("cost_spike:"));
        assert_eq!(candidates[0].suggested_action_type, "model_change");
        assert!(candidates[0].evidence.contains("3.0x increase"));
    }

    #[test]
    fn generates_candidate_for_failure_pattern() {
        let triggers = vec![TriggerReason::FailurePattern {
            tool: "bash".to_string(),
            failure_count: 4,
        }];
        let candidates = generate_candidates(&triggers, &empty_summary(), &test_context());

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].trigger, "failure_pattern:bash:4");
        assert_eq!(candidates[0].suggested_action_type, "warning");
        assert!(candidates[0].evidence.contains("bash"));
    }

    #[test]
    fn generates_candidate_for_override_pattern() {
        let triggers = vec![TriggerReason::OverridePattern {
            override_type: "strict_mode".to_string(),
            count: 5,
        }];
        let candidates = generate_candidates(&triggers, &empty_summary(), &test_context());

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].trigger, "override_pattern:strict_mode:5");
        assert_eq!(candidates[0].suggested_action_type, "mode_create");
        assert!(candidates[0].context.contains("Available modes"));
    }

    #[test]
    fn generates_candidate_for_new_session() {
        let triggers = vec![TriggerReason::NewSession];
        let candidates = generate_candidates(&triggers, &empty_summary(), &test_context());

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].trigger, "new_session");
        assert_eq!(candidates[0].suggested_action_type, "review");
    }

    #[test]
    fn filters_dismissed_patterns() {
        let triggers = vec![
            TriggerReason::RejectionsAccumulated { count: 5 },
            TriggerReason::NewSession,
        ];
        let mut ctx = test_context();
        ctx.dismissed_patterns = vec!["rejections_accumulated".to_string()];

        let candidates = generate_candidates(&triggers, &empty_summary(), &ctx);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].trigger, "new_session");
    }

    #[test]
    fn multiple_triggers_produce_multiple_candidates() {
        let triggers = vec![
            TriggerReason::RejectionsAccumulated { count: 3 },
            TriggerReason::NewSession,
            TriggerReason::FailurePattern {
                tool: "code_edit".to_string(),
                failure_count: 3,
            },
        ];
        let candidates = generate_candidates(&triggers, &empty_summary(), &test_context());

        assert_eq!(candidates.len(), 3);
    }

    #[test]
    fn empty_triggers_produce_no_candidates() {
        let candidates = generate_candidates(&[], &empty_summary(), &test_context());
        assert!(candidates.is_empty());
    }

    #[test]
    fn build_mode_create_action_constructs_correctly() {
        let action = build_mode_create_action(
            "db_mode",
            "Database operations",
            "You are a database expert",
            Some("gpt-4"),
            &["sql_query", "migration"],
        );
        assert_eq!(
            action,
            RecommendationAction::ModeCreate {
                name: "db_mode".to_string(),
                description: "Database operations".to_string(),
                system_prompt: "You are a database expert".to_string(),
                default_model: Some("gpt-4".to_string()),
                allowed_tools: vec!["sql_query".to_string(), "migration".to_string()],
            }
        );
    }

    #[test]
    fn build_mode_edit_action_constructs_correctly() {
        let changes = ModeChanges {
            description: Some("Updated description".to_string()),
            system_prompt: None,
            default_model: None,
            allowed_tools: None,
        };
        let action = build_mode_edit_action("strict", changes.clone());
        assert_eq!(
            action,
            RecommendationAction::ModeEdit {
                mode_name: "strict".to_string(),
                changes,
            }
        );
    }

    #[test]
    fn build_model_change_action_constructs_correctly() {
        let action = build_model_change_action("coder", "gpt-4", "gpt-3.5");
        assert_eq!(
            action,
            RecommendationAction::ModelChange {
                role: "coder".to_string(),
                from_model: "gpt-4".to_string(),
                to_model: "gpt-3.5".to_string(),
            }
        );
    }

    #[test]
    fn build_config_change_action_constructs_correctly() {
        let action = build_config_change_action("max_tokens", "4096", "8192");
        assert_eq!(
            action,
            RecommendationAction::ConfigChange {
                key: "max_tokens".to_string(),
                old_value: "4096".to_string(),
                new_value: "8192".to_string(),
            }
        );
    }

    #[test]
    fn build_prompt_edit_action_constructs_correctly() {
        let action = build_prompt_edit_action("strict", "old text", "new text");
        assert_eq!(
            action,
            RecommendationAction::PromptEdit {
                mode_name: "strict".to_string(),
                old_fragment: "old text".to_string(),
                new_fragment: "new text".to_string(),
            }
        );
    }

    #[test]
    fn decision_candidate_serializes_to_json() {
        let candidate = DecisionCandidate {
            trigger: "new_session".to_string(),
            suggested_action_type: "review".to_string(),
            evidence: "test evidence".to_string(),
            context: "test context".to_string(),
        };
        let json = serde_json::to_string(&candidate).unwrap();
        let deserialized: DecisionCandidate = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.trigger, candidate.trigger);
        assert_eq!(
            deserialized.suggested_action_type,
            candidate.suggested_action_type
        );
    }
}
