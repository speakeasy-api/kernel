use uuid::Uuid;

use super::types::{EngagementLevel, Task, TaskStatus};

/// Resolve the effective engagement level for a task.
/// Per-task override takes precedence over the default.
pub fn resolve_engagement(task: &Task, default: EngagementLevel) -> EngagementLevel {
    task.engagement_override.unwrap_or(default)
}

/// Determines if a gate check is needed before proceeding after a task reaches the given status.
/// Returns true if the system should pause and wait for user review/input.
pub fn needs_gate_check(engagement: EngagementLevel, task_status: TaskStatus) -> bool {
    match engagement {
        EngagementLevel::Autonomous => false,
        EngagementLevel::ReviewGates => matches!(task_status, TaskStatus::Review),
        EngagementLevel::Collaborative => {
            matches!(task_status, TaskStatus::InProgress | TaskStatus::Review)
        }
    }
}

/// Returns a human-readable description of the engagement level behavior.
pub fn describe_engagement(level: EngagementLevel) -> &'static str {
    match level {
        EngagementLevel::Autonomous => {
            "Agents work through the entire task tree, only stopping for explicit blocks"
        }
        EngagementLevel::ReviewGates => {
            "Agents pause after each task for user review before proceeding"
        }
        EngagementLevel::Collaborative => {
            "Agents work on one task at a time, discussing approach with user"
        }
    }
}

/// After a task transitions, determine the scheduling action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulingAction {
    /// Continue scheduling — pick next tasks
    Continue,
    /// Pause — wait for user review/approval of this task
    WaitForReview { task_id: Uuid },
    /// Pause — wait for user to discuss approach for next task
    WaitForCollaboration { task_id: Uuid },
}

pub fn post_transition_action(
    task: &Task,
    new_status: TaskStatus,
    default_engagement: EngagementLevel,
) -> SchedulingAction {
    let engagement = resolve_engagement(task, default_engagement);

    match (engagement, new_status) {
        (EngagementLevel::Autonomous, _) => SchedulingAction::Continue,
        (EngagementLevel::ReviewGates, TaskStatus::Review) => {
            SchedulingAction::WaitForReview { task_id: task.id }
        }
        (EngagementLevel::ReviewGates, _) => SchedulingAction::Continue,
        (EngagementLevel::Collaborative, TaskStatus::InProgress) => {
            SchedulingAction::WaitForCollaboration { task_id: task.id }
        }
        (EngagementLevel::Collaborative, TaskStatus::Review) => {
            SchedulingAction::WaitForReview { task_id: task.id }
        }
        (EngagementLevel::Collaborative, _) => SchedulingAction::Continue,
    }
}

/// Returns the effective max concurrent tasks for the given engagement level.
/// Collaborative mode forces max_concurrent to 1.
pub fn effective_max_concurrent(engagement: EngagementLevel, configured_max: usize) -> usize {
    match engagement {
        EngagementLevel::Collaborative => 1,
        _ => configured_max,
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::tasks::types::{Priority, Task};

    fn sample_task(engagement_override: Option<EngagementLevel>) -> Task {
        let now = Utc::now();
        Task {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            title: "Task".to_string(),
            description: "Description".to_string(),
            status: TaskStatus::Pending,
            priority: Priority::Medium,
            assigned_agent: None,
            parent_task: None,
            depends_on: Vec::new(),
            worktree_branch: None,
            base_ref: "main".to_string(),
            base_commit: "deadbeef".to_string(),
            merge_target_ref: "main".to_string(),
            outcome: None,
            engagement_override,
            cost_usd: 0.0,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn resolve_engagement_prefers_override() {
        let task = sample_task(Some(EngagementLevel::Collaborative));
        assert_eq!(
            resolve_engagement(&task, EngagementLevel::Autonomous),
            EngagementLevel::Collaborative
        );
    }

    #[test]
    fn resolve_engagement_falls_back_to_default() {
        let task = sample_task(None);
        assert_eq!(
            resolve_engagement(&task, EngagementLevel::ReviewGates),
            EngagementLevel::ReviewGates
        );
    }

    #[test]
    fn needs_gate_check_matches_engagement_rules() {
        let statuses = [
            TaskStatus::Pending,
            TaskStatus::InProgress,
            TaskStatus::Blocked,
            TaskStatus::Review,
            TaskStatus::Done,
        ];
        for status in statuses {
            assert!(!needs_gate_check(EngagementLevel::Autonomous, status));
        }

        assert!(needs_gate_check(
            EngagementLevel::ReviewGates,
            TaskStatus::Review
        ));
        assert!(!needs_gate_check(
            EngagementLevel::ReviewGates,
            TaskStatus::InProgress
        ));

        assert!(needs_gate_check(
            EngagementLevel::Collaborative,
            TaskStatus::InProgress
        ));
        assert!(needs_gate_check(
            EngagementLevel::Collaborative,
            TaskStatus::Review
        ));
        assert!(!needs_gate_check(
            EngagementLevel::Collaborative,
            TaskStatus::Pending
        ));
    }

    #[test]
    fn post_transition_action_matches_expected_behavior() {
        let autonomous = sample_task(Some(EngagementLevel::Autonomous));
        assert_eq!(
            post_transition_action(
                &autonomous,
                TaskStatus::Review,
                EngagementLevel::Collaborative
            ),
            SchedulingAction::Continue
        );

        let review_gates = sample_task(Some(EngagementLevel::ReviewGates));
        assert_eq!(
            post_transition_action(
                &review_gates,
                TaskStatus::Review,
                EngagementLevel::Autonomous
            ),
            SchedulingAction::WaitForReview {
                task_id: review_gates.id
            }
        );

        let collaborative = sample_task(Some(EngagementLevel::Collaborative));
        assert_eq!(
            post_transition_action(
                &collaborative,
                TaskStatus::InProgress,
                EngagementLevel::Autonomous
            ),
            SchedulingAction::WaitForCollaboration {
                task_id: collaborative.id
            }
        );
        assert_eq!(
            post_transition_action(
                &collaborative,
                TaskStatus::Review,
                EngagementLevel::Autonomous
            ),
            SchedulingAction::WaitForReview {
                task_id: collaborative.id
            }
        );
    }

    #[test]
    fn effective_max_concurrent_forces_collaborative_to_one() {
        assert_eq!(
            effective_max_concurrent(EngagementLevel::Collaborative, 8),
            1
        );
        assert_eq!(effective_max_concurrent(EngagementLevel::Autonomous, 8), 8);
        assert_eq!(effective_max_concurrent(EngagementLevel::ReviewGates, 8), 8);
    }
}
