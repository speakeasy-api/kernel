use std::collections::HashMap;

use rusqlite::Connection;

use super::types::{Event, EventData};
use crate::db::queries::{get_ux_state, update_ux_state};

/// Why the UX agent should wake up.
#[derive(Debug, Clone, PartialEq)]
pub enum TriggerReason {
    RejectionsAccumulated(u32),
    NewSession,
    CostSpike {
        current_rate: f64,
        previous_rate: f64,
    },
    FailurePattern {
        tool: String,
        count: u32,
    },
    OverridePattern {
        mode: String,
        count: u32,
    },
}

/// Persisted position so the accumulator can resume after restart.
#[derive(Debug, Clone, Default)]
pub struct AccumulatorCursor {
    pub last_event_id: Option<String>,
    pub last_event_at: Option<String>,
}

/// In-memory event buffer that triggers UX agent wake-ups when thresholds are
/// crossed.  Counters are reset after the UX agent processes them.
pub struct EventAccumulator {
    rejection_count: u32,
    last_cost_rate: f64,
    override_counts: HashMap<String, u32>,
    failure_counts: HashMap<String, u32>,
    cursor: AccumulatorCursor,
}

const REJECTION_THRESHOLD: u32 = 3;
const COST_SPIKE_MULTIPLIER: f64 = 2.0;
const FAILURE_THRESHOLD: u32 = 3;
const OVERRIDE_THRESHOLD: u32 = 3;

impl EventAccumulator {
    pub fn new() -> Self {
        Self {
            rejection_count: 0,
            last_cost_rate: 0.0,
            override_counts: HashMap::new(),
            failure_counts: HashMap::new(),
            cursor: AccumulatorCursor::default(),
        }
    }

    /// Feed an event into the accumulator. Returns `Some(reason)` when a
    /// threshold is crossed, `None` for routine events.
    pub fn accumulate(&mut self, event: &Event) -> Option<TriggerReason> {
        // Track cursor position
        self.cursor.last_event_id = Some(event.metadata.id.to_string());
        self.cursor.last_event_at = Some(event.metadata.timestamp.to_rfc3339());

        match &event.data {
            // --- Rejections ---
            EventData::PlanRejected { .. }
            | EventData::DiffRejected { .. }
            | EventData::HunkRejected { .. } => {
                self.rejection_count += 1;
                if self.rejection_count >= REJECTION_THRESHOLD {
                    return Some(TriggerReason::RejectionsAccumulated(self.rejection_count));
                }
            }

            // --- New session ---
            EventData::PromptSubmitted { .. } => {
                // A PromptSubmitted with no prior cursor means fresh session
                if self.cursor.last_event_id.as_deref() == Some(&event.metadata.id.to_string())
                    && self.last_cost_rate == 0.0
                    && self.rejection_count == 0
                    && self.override_counts.is_empty()
                    && self.failure_counts.is_empty()
                {
                    return Some(TriggerReason::NewSession);
                }
            }

            // --- Cost spikes ---
            EventData::CostIncurred { cost_usd, .. } => {
                let current = *cost_usd;
                if self.last_cost_rate > 0.0
                    && current >= self.last_cost_rate * COST_SPIKE_MULTIPLIER
                {
                    let reason = TriggerReason::CostSpike {
                        current_rate: current,
                        previous_rate: self.last_cost_rate,
                    };
                    self.last_cost_rate = current;
                    return Some(reason);
                }
                self.last_cost_rate = current;
            }

            // --- Tool failure patterns ---
            EventData::ToolFailed { tool, .. } => {
                let count = self.failure_counts.entry(tool.clone()).or_insert(0);
                *count += 1;
                if *count >= FAILURE_THRESHOLD {
                    return Some(TriggerReason::FailurePattern {
                        tool: tool.clone(),
                        count: *count,
                    });
                }
            }

            // --- Mode override patterns ---
            EventData::ModeOverridden { to_mode, .. } => {
                let count = self.override_counts.entry(to_mode.clone()).or_insert(0);
                *count += 1;
                if *count >= OVERRIDE_THRESHOLD {
                    return Some(TriggerReason::OverridePattern {
                        mode: to_mode.clone(),
                        count: *count,
                    });
                }
            }

            // Routine events — no trigger
            _ => {}
        }

        None
    }

    /// Save the cursor to the `ux_agent_state` table so we can resume after
    /// restart.
    pub fn persist_cursor(&self, conn: &Connection) -> Result<(), rusqlite::Error> {
        if let (Some(id), Some(at)) = (&self.cursor.last_event_id, &self.cursor.last_event_at) {
            update_ux_state(conn, "accumulator", id, at)?;
        }
        Ok(())
    }

    /// Load a previously-persisted cursor from the database.
    pub fn restore_cursor(conn: &Connection) -> Result<AccumulatorCursor, rusqlite::Error> {
        match get_ux_state(conn, "accumulator")? {
            Some(state) => Ok(AccumulatorCursor {
                last_event_id: state.last_event_id,
                last_event_at: state.last_event_at,
            }),
            None => Ok(AccumulatorCursor::default()),
        }
    }

    /// Clear all counters after the UX agent has processed the trigger.
    pub fn reset(&mut self) {
        self.rejection_count = 0;
        self.last_cost_rate = 0.0;
        self.override_counts.clear();
        self.failure_counts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations;
    use crate::events::{EventData, EventMetadata};
    use chrono::Utc;
    use uuid::Uuid;

    fn make_event(data: EventData) -> Event {
        Event {
            metadata: EventMetadata {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                session_id: Uuid::new_v4(),
                agent_id: None,
            },
            data,
        }
    }

    // --- Rejection tracking ---

    #[test]
    fn no_trigger_below_rejection_threshold() {
        let mut acc = EventAccumulator::new();
        let tid = Uuid::new_v4();

        assert!(acc
            .accumulate(&make_event(EventData::PlanRejected {
                task_id: tid,
                feedback: "nope".into(),
            }))
            .is_none());

        assert!(acc
            .accumulate(&make_event(EventData::DiffRejected {
                task_id: tid,
                branch: "feat".into(),
                feedback: "nah".into(),
            }))
            .is_none());
    }

    #[test]
    fn triggers_at_rejection_threshold() {
        let mut acc = EventAccumulator::new();
        let tid = Uuid::new_v4();

        for _ in 0..2 {
            acc.accumulate(&make_event(EventData::PlanRejected {
                task_id: tid,
                feedback: "no".into(),
            }));
        }

        let result = acc.accumulate(&make_event(EventData::HunkRejected {
            task_id: tid,
            file: "main.rs".into(),
            hunk_index: 0,
            reason: "bad".into(),
        }));

        assert_eq!(result, Some(TriggerReason::RejectionsAccumulated(3)));
    }

    // --- Cost spike detection ---

    #[test]
    fn no_spike_on_first_cost_event() {
        let mut acc = EventAccumulator::new();
        let aid = Uuid::new_v4();

        let result = acc.accumulate(&make_event(EventData::CostIncurred {
            agent_id: aid,
            model: "m".into(),
            cost_usd: 0.10,
        }));
        assert!(result.is_none());
    }

    #[test]
    fn no_spike_on_small_increase() {
        let mut acc = EventAccumulator::new();
        let aid = Uuid::new_v4();

        acc.accumulate(&make_event(EventData::CostIncurred {
            agent_id: aid,
            model: "m".into(),
            cost_usd: 0.10,
        }));

        let result = acc.accumulate(&make_event(EventData::CostIncurred {
            agent_id: aid,
            model: "m".into(),
            cost_usd: 0.15,
        }));
        assert!(result.is_none());
    }

    #[test]
    fn detects_cost_spike() {
        let mut acc = EventAccumulator::new();
        let aid = Uuid::new_v4();

        acc.accumulate(&make_event(EventData::CostIncurred {
            agent_id: aid,
            model: "m".into(),
            cost_usd: 0.10,
        }));

        let result = acc.accumulate(&make_event(EventData::CostIncurred {
            agent_id: aid,
            model: "m".into(),
            cost_usd: 0.25,
        }));

        assert_eq!(
            result,
            Some(TriggerReason::CostSpike {
                current_rate: 0.25,
                previous_rate: 0.10,
            })
        );
    }

    // --- Failure pattern detection ---

    #[test]
    fn no_trigger_below_failure_threshold() {
        let mut acc = EventAccumulator::new();
        let aid = Uuid::new_v4();

        for _ in 0..2 {
            assert!(acc
                .accumulate(&make_event(EventData::ToolFailed {
                    agent_id: aid,
                    tool: "read_file".into(),
                    error: "not found".into(),
                }))
                .is_none());
        }
    }

    #[test]
    fn triggers_failure_pattern() {
        let mut acc = EventAccumulator::new();
        let aid = Uuid::new_v4();

        for _ in 0..2 {
            acc.accumulate(&make_event(EventData::ToolFailed {
                agent_id: aid,
                tool: "read_file".into(),
                error: "err".into(),
            }));
        }

        let result = acc.accumulate(&make_event(EventData::ToolFailed {
            agent_id: aid,
            tool: "read_file".into(),
            error: "err".into(),
        }));

        assert_eq!(
            result,
            Some(TriggerReason::FailurePattern {
                tool: "read_file".into(),
                count: 3,
            })
        );
    }

    #[test]
    fn different_tools_tracked_independently() {
        let mut acc = EventAccumulator::new();
        let aid = Uuid::new_v4();

        for _ in 0..2 {
            acc.accumulate(&make_event(EventData::ToolFailed {
                agent_id: aid,
                tool: "read_file".into(),
                error: "err".into(),
            }));
        }

        // Different tool — should not cross threshold
        let result = acc.accumulate(&make_event(EventData::ToolFailed {
            agent_id: aid,
            tool: "write_file".into(),
            error: "err".into(),
        }));
        assert!(result.is_none());
    }

    // --- Override pattern detection ---

    #[test]
    fn triggers_override_pattern() {
        let mut acc = EventAccumulator::new();

        for _ in 0..2 {
            acc.accumulate(&make_event(EventData::ModeOverridden {
                from_mode: "auto".into(),
                to_mode: "manual".into(),
            }));
        }

        let result = acc.accumulate(&make_event(EventData::ModeOverridden {
            from_mode: "auto".into(),
            to_mode: "manual".into(),
        }));

        assert_eq!(
            result,
            Some(TriggerReason::OverridePattern {
                mode: "manual".into(),
                count: 3,
            })
        );
    }

    #[test]
    fn different_modes_tracked_independently() {
        let mut acc = EventAccumulator::new();

        for _ in 0..2 {
            acc.accumulate(&make_event(EventData::ModeOverridden {
                from_mode: "auto".into(),
                to_mode: "manual".into(),
            }));
        }

        let result = acc.accumulate(&make_event(EventData::ModeOverridden {
            from_mode: "auto".into(),
            to_mode: "debug".into(),
        }));
        assert!(result.is_none());
    }

    // --- Routine events don't trigger ---

    #[test]
    fn routine_success_events_do_not_trigger() {
        let mut acc = EventAccumulator::new();
        let aid = Uuid::new_v4();
        let tid = Uuid::new_v4();

        let routine_events = vec![
            EventData::ToolSucceeded {
                agent_id: aid,
                tool: "read_file".into(),
                duration_ms: 10,
            },
            EventData::AgentCompleted {
                agent_id: aid,
                summary: "done".into(),
                token_usage: crate::events::TokenMetrics {
                    input: 100,
                    output: 50,
                },
            },
            EventData::TaskCompleted {
                task_id: tid,
                summary: "done".into(),
                diff_stat: crate::events::DiffStat {
                    files_changed: 1,
                    insertions: 5,
                    deletions: 2,
                },
            },
            EventData::PlanAccepted { task_id: tid },
            EventData::DiffAccepted {
                task_id: tid,
                branch: "feat".into(),
            },
        ];

        for data in routine_events {
            assert!(acc.accumulate(&make_event(data)).is_none());
        }
    }

    // --- Reset ---

    #[test]
    fn reset_clears_counters() {
        let mut acc = EventAccumulator::new();
        let tid = Uuid::new_v4();
        let aid = Uuid::new_v4();

        // Accumulate some state
        acc.accumulate(&make_event(EventData::PlanRejected {
            task_id: tid,
            feedback: "no".into(),
        }));
        acc.accumulate(&make_event(EventData::CostIncurred {
            agent_id: aid,
            model: "m".into(),
            cost_usd: 0.10,
        }));
        acc.accumulate(&make_event(EventData::ToolFailed {
            agent_id: aid,
            tool: "t".into(),
            error: "e".into(),
        }));
        acc.accumulate(&make_event(EventData::ModeOverridden {
            from_mode: "a".into(),
            to_mode: "b".into(),
        }));

        acc.reset();

        // After reset, thresholds start from zero again
        assert!(acc
            .accumulate(&make_event(EventData::PlanRejected {
                task_id: tid,
                feedback: "no".into(),
            }))
            .is_none());

        // Cost spike needs a baseline first, then a 2x jump
        acc.accumulate(&make_event(EventData::CostIncurred {
            agent_id: aid,
            model: "m".into(),
            cost_usd: 0.10,
        }));
        assert!(acc
            .accumulate(&make_event(EventData::CostIncurred {
                agent_id: aid,
                model: "m".into(),
                cost_usd: 0.15,
            }))
            .is_none());
    }

    // --- Cursor persistence ---

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn cursor_persist_and_restore() {
        let conn = setup_db();
        let mut acc = EventAccumulator::new();

        // Feed one event so cursor is populated
        acc.accumulate(&make_event(EventData::ToolSucceeded {
            agent_id: Uuid::new_v4(),
            tool: "t".into(),
            duration_ms: 1,
        }));

        acc.persist_cursor(&conn).unwrap();

        let restored = EventAccumulator::restore_cursor(&conn).unwrap();
        assert_eq!(restored.last_event_id, acc.cursor.last_event_id);
        assert_eq!(restored.last_event_at, acc.cursor.last_event_at);
    }

    #[test]
    fn restore_cursor_returns_default_when_empty() {
        let conn = setup_db();
        let cursor = EventAccumulator::restore_cursor(&conn).unwrap();
        assert!(cursor.last_event_id.is_none());
        assert!(cursor.last_event_at.is_none());
    }

    // --- NewSession trigger ---

    #[test]
    fn first_event_in_fresh_accumulator_triggers_new_session() {
        let mut acc = EventAccumulator::new();

        let result = acc.accumulate(&make_event(EventData::PromptSubmitted {
            prompt: "hello".into(),
        }));

        assert_eq!(result, Some(TriggerReason::NewSession));
    }

    #[test]
    fn prompt_submitted_after_activity_does_not_trigger_new_session() {
        let mut acc = EventAccumulator::new();
        let aid = Uuid::new_v4();

        // Some prior activity
        acc.accumulate(&make_event(EventData::CostIncurred {
            agent_id: aid,
            model: "m".into(),
            cost_usd: 0.01,
        }));

        let result = acc.accumulate(&make_event(EventData::PromptSubmitted {
            prompt: "hello".into(),
        }));

        assert!(result.is_none());
    }
}
