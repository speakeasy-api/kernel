use tracing::{debug, info, instrument};

/// Threshold for accumulated rejection events (plan, diff, or hunk rejections).
pub const REJECTION_THRESHOLD: usize = 3;

/// Cost must exceed baseline by this multiplier to trigger a cost spike.
pub const COST_SPIKE_MULTIPLIER: f64 = 2.0;

/// Number of failures for a single tool before a failure pattern triggers.
pub const FAILURE_PATTERN_THRESHOLD: usize = 3;

/// Number of overrides of the same type before an override pattern triggers.
pub const OVERRIDE_PATTERN_THRESHOLD: usize = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum TriggerReason {
    RejectionsAccumulated { count: usize },
    NewSession,
    CostSpike { current_rate_usd: f64, baseline_rate_usd: f64 },
    FailurePattern { tool: String, failure_count: usize },
    OverridePattern { override_type: String, count: usize },
}

#[derive(Debug, Clone, Default)]
pub struct EventSummary {
    pub rejection_count: usize,
    pub is_new_session: bool,
    pub recent_cost_usd: f64,
    pub baseline_cost_usd: f64,
    pub tool_failures: Vec<(String, usize)>,
    pub mode_overrides: Vec<(String, usize)>,
    pub has_new_events: bool,
}

/// Build an `EventSummary` from raw event counts.
///
/// In production this will be called with data from the events query layer.
#[instrument(skip(tool_failures, mode_overrides))]
pub fn build_event_summary(
    rejection_count: usize,
    is_new_session: bool,
    recent_cost_usd: f64,
    baseline_cost_usd: f64,
    tool_failures: Vec<(String, usize)>,
    mode_overrides: Vec<(String, usize)>,
    has_new_events: bool,
) -> EventSummary {
    debug!(
        rejection_count,
        is_new_session,
        recent_cost_usd,
        baseline_cost_usd,
        tool_failure_count = tool_failures.len(),
        mode_override_count = mode_overrides.len(),
        has_new_events,
        "building event summary"
    );
    EventSummary {
        rejection_count,
        is_new_session,
        recent_cost_usd,
        baseline_cost_usd,
        tool_failures,
        mode_overrides,
        has_new_events,
    }
}

/// Evaluate which triggers have fired given an event summary.
///
/// Returns all triggers that fired -- multiple may fire simultaneously.
/// Returns an empty vec when `has_new_events` is false.
#[instrument(skip(summary), fields(has_new_events = summary.has_new_events, rejection_count = summary.rejection_count))]
pub fn evaluate_triggers(summary: &EventSummary) -> Vec<TriggerReason> {
    debug!(
        has_new_events = summary.has_new_events,
        rejection_count = summary.rejection_count,
        is_new_session = summary.is_new_session,
        "evaluating triggers"
    );
    if !summary.has_new_events {
        debug!("no new events, skipping trigger evaluation");
        return Vec::new();
    }

    let mut triggers = Vec::new();

    if summary.rejection_count >= REJECTION_THRESHOLD {
        triggers.push(TriggerReason::RejectionsAccumulated {
            count: summary.rejection_count,
        });
    }

    if summary.is_new_session {
        triggers.push(TriggerReason::NewSession);
    }

    if summary.baseline_cost_usd > 0.0
        && summary.recent_cost_usd > summary.baseline_cost_usd * COST_SPIKE_MULTIPLIER
    {
        triggers.push(TriggerReason::CostSpike {
            current_rate_usd: summary.recent_cost_usd,
            baseline_rate_usd: summary.baseline_cost_usd,
        });
    }

    for (tool, count) in &summary.tool_failures {
        if *count >= FAILURE_PATTERN_THRESHOLD {
            triggers.push(TriggerReason::FailurePattern {
                tool: tool.clone(),
                failure_count: *count,
            });
        }
    }

    for (override_type, count) in &summary.mode_overrides {
        if *count >= OVERRIDE_PATTERN_THRESHOLD {
            triggers.push(TriggerReason::OverridePattern {
                override_type: override_type.clone(),
                count: *count,
            });
        }
    }

    for trigger in &triggers {
        info!(?trigger, "trigger fired");
    }
    if triggers.is_empty() {
        debug!("no triggers fired");
    }

    triggers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_triggers_when_no_new_events() {
        let summary = EventSummary {
            rejection_count: 10,
            is_new_session: true,
            has_new_events: false,
            ..Default::default()
        };
        assert!(evaluate_triggers(&summary).is_empty());
    }

    #[test]
    fn rejections_accumulated() {
        let summary = EventSummary {
            rejection_count: 3,
            has_new_events: true,
            ..Default::default()
        };
        let triggers = evaluate_triggers(&summary);
        assert_eq!(triggers, vec![TriggerReason::RejectionsAccumulated { count: 3 }]);
    }

    #[test]
    fn rejections_below_threshold() {
        let summary = EventSummary {
            rejection_count: 2,
            has_new_events: true,
            ..Default::default()
        };
        assert!(evaluate_triggers(&summary).is_empty());
    }

    #[test]
    fn new_session_trigger() {
        let summary = EventSummary {
            is_new_session: true,
            has_new_events: true,
            ..Default::default()
        };
        let triggers = evaluate_triggers(&summary);
        assert_eq!(triggers, vec![TriggerReason::NewSession]);
    }

    #[test]
    fn cost_spike_trigger() {
        let summary = EventSummary {
            baseline_cost_usd: 1.0,
            recent_cost_usd: 2.5,
            has_new_events: true,
            ..Default::default()
        };
        let triggers = evaluate_triggers(&summary);
        assert_eq!(
            triggers,
            vec![TriggerReason::CostSpike {
                current_rate_usd: 2.5,
                baseline_rate_usd: 1.0,
            }]
        );
    }

    #[test]
    fn cost_spike_not_triggered_when_baseline_zero() {
        let summary = EventSummary {
            baseline_cost_usd: 0.0,
            recent_cost_usd: 100.0,
            has_new_events: true,
            ..Default::default()
        };
        assert!(evaluate_triggers(&summary).is_empty());
    }

    #[test]
    fn cost_spike_not_triggered_at_exact_double() {
        let summary = EventSummary {
            baseline_cost_usd: 1.0,
            recent_cost_usd: 2.0,
            has_new_events: true,
            ..Default::default()
        };
        assert!(evaluate_triggers(&summary).is_empty());
    }

    #[test]
    fn failure_pattern_trigger() {
        let summary = EventSummary {
            tool_failures: vec![
                ("code_edit".to_string(), 3),
                ("file_read".to_string(), 1),
            ],
            has_new_events: true,
            ..Default::default()
        };
        let triggers = evaluate_triggers(&summary);
        assert_eq!(
            triggers,
            vec![TriggerReason::FailurePattern {
                tool: "code_edit".to_string(),
                failure_count: 3,
            }]
        );
    }

    #[test]
    fn override_pattern_trigger() {
        let summary = EventSummary {
            mode_overrides: vec![("strict_mode".to_string(), 4)],
            has_new_events: true,
            ..Default::default()
        };
        let triggers = evaluate_triggers(&summary);
        assert_eq!(
            triggers,
            vec![TriggerReason::OverridePattern {
                override_type: "strict_mode".to_string(),
                count: 4,
            }]
        );
    }

    #[test]
    fn multiple_triggers_fire_simultaneously() {
        let summary = EventSummary {
            rejection_count: 5,
            is_new_session: true,
            baseline_cost_usd: 1.0,
            recent_cost_usd: 3.0,
            tool_failures: vec![("bash".to_string(), 3)],
            mode_overrides: vec![("auto_approve".to_string(), 3)],
            has_new_events: true,
        };
        let triggers = evaluate_triggers(&summary);
        assert_eq!(triggers.len(), 5);
        assert_eq!(triggers[0], TriggerReason::RejectionsAccumulated { count: 5 });
        assert_eq!(triggers[1], TriggerReason::NewSession);
        assert_eq!(
            triggers[2],
            TriggerReason::CostSpike {
                current_rate_usd: 3.0,
                baseline_rate_usd: 1.0,
            }
        );
        assert_eq!(
            triggers[3],
            TriggerReason::FailurePattern {
                tool: "bash".to_string(),
                failure_count: 3,
            }
        );
        assert_eq!(
            triggers[4],
            TriggerReason::OverridePattern {
                override_type: "auto_approve".to_string(),
                count: 3,
            }
        );
    }
}
