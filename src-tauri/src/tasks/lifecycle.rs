use sqlx::SqlitePool;
use uuid::Uuid;

use super::db;
use super::types::{TaskOutcome, TaskStatus};

#[derive(Debug, Clone)]
pub struct Transition {
    pub from: TaskStatus,
    pub to: TaskStatus,
    pub outcome: Option<TaskOutcome>,
}

#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("Invalid transition from {from:?} to {to:?}")]
    InvalidTransition { from: TaskStatus, to: TaskStatus },
    #[error("Transition to Done requires an outcome")]
    MissingOutcome,
    #[error("Database error: {0}")]
    DbError(#[from] sqlx::Error),
    #[error("Task not found: {0}")]
    TaskNotFound(Uuid),
}

pub fn validate_transition(
    from: TaskStatus,
    to: TaskStatus,
    outcome: Option<&TaskOutcome>,
) -> Result<(), LifecycleError> {
    let valid = matches!(
        (from, to),
        (TaskStatus::Pending, TaskStatus::InProgress)
            | (TaskStatus::Pending, TaskStatus::Blocked)
            | (TaskStatus::InProgress, TaskStatus::Review)
            | (TaskStatus::InProgress, TaskStatus::Blocked)
            | (TaskStatus::InProgress, TaskStatus::Done)
            | (TaskStatus::Blocked, TaskStatus::Pending)
            | (TaskStatus::Review, TaskStatus::Done)
            | (TaskStatus::Review, TaskStatus::InProgress)
            | (_, TaskStatus::Done)
    );

    if !valid {
        return Err(LifecycleError::InvalidTransition { from, to });
    }

    if to == TaskStatus::Done && outcome.is_none() {
        return Err(LifecycleError::MissingOutcome);
    }

    Ok(())
}

pub async fn apply_transition(
    pool: &SqlitePool,
    task_id: Uuid,
    to: TaskStatus,
    outcome: Option<&TaskOutcome>,
) -> Result<(), LifecycleError> {
    let task = db::get_task(pool, task_id)
        .await?
        .ok_or(LifecycleError::TaskNotFound(task_id))?;
    validate_transition(task.status, to, outcome)?;
    db::update_task_status(pool, task_id, to, outcome).await?;
    Ok(())
}

pub async fn cascade_unblock(
    pool: &SqlitePool,
    completed_task_id: Uuid,
) -> Result<Vec<Uuid>, LifecycleError> {
    let dependents = db::find_dependents(pool, completed_task_id).await?;
    let mut unblocked = Vec::new();

    for dependent_task_id in dependents {
        let task = db::get_task(pool, dependent_task_id)
            .await?
            .ok_or(LifecycleError::TaskNotFound(dependent_task_id))?;

        if task.status != TaskStatus::Blocked {
            continue;
        }

        let mut all_done = true;
        for dependency_id in task.depends_on {
            let dependency = db::get_task(pool, dependency_id)
                .await?
                .ok_or(LifecycleError::TaskNotFound(dependency_id))?;
            if dependency.status != TaskStatus::Done {
                all_done = false;
                break;
            }
        }

        if all_done {
            db::update_task_status(pool, dependent_task_id, TaskStatus::Pending, None).await?;
            unblocked.push(dependent_task_id);
        }
    }

    Ok(unblocked)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::db::test_pool;
    use crate::tasks::db;
    use crate::tasks::types::{DiffStat, Priority, Task};

    fn done_outcome() -> TaskOutcome {
        TaskOutcome::Success {
            summary: "ok".to_string(),
            diff_stat: DiffStat {
                files_changed: 1,
                insertions: 1,
                deletions: 0,
            },
        }
    }

    fn sample_task(
        session_id: Uuid,
        title: &str,
        status: TaskStatus,
        depends_on: Vec<Uuid>,
    ) -> Task {
        let now = Utc::now();
        Task {
            id: Uuid::new_v4(),
            session_id,
            title: title.to_string(),
            description: String::new(),
            status,
            priority: Priority::Medium,
            assigned_agent: None,
            parent_task: None,
            depends_on,
            worktree_branch: None,
            base_ref: "main".to_string(),
            base_commit: "deadbeef".to_string(),
            merge_target_ref: "main".to_string(),
            outcome: if status == TaskStatus::Done {
                Some(done_outcome())
            } else {
                None
            },
            engagement_override: None,
            cost_usd: 0.0,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn validate_transition_accepts_legal_transitions() {
        let legal = [
            (TaskStatus::Pending, TaskStatus::InProgress),
            (TaskStatus::Pending, TaskStatus::Blocked),
            (TaskStatus::InProgress, TaskStatus::Review),
            (TaskStatus::InProgress, TaskStatus::Blocked),
            (TaskStatus::InProgress, TaskStatus::Done),
            (TaskStatus::Blocked, TaskStatus::Pending),
            (TaskStatus::Review, TaskStatus::Done),
            (TaskStatus::Review, TaskStatus::InProgress),
        ];

        for (from, to) in legal {
            let outcome = if to == TaskStatus::Done {
                Some(done_outcome())
            } else {
                None
            };
            assert!(validate_transition(from, to, outcome.as_ref()).is_ok());
        }

        assert!(validate_transition(
            TaskStatus::Pending,
            TaskStatus::Done,
            Some(&TaskOutcome::Failure {
                reason: "hard stop".to_string()
            }),
        )
        .is_ok());
    }

    #[test]
    fn validate_transition_rejects_illegal_transitions() {
        let illegal = [
            (TaskStatus::Done, TaskStatus::Pending),
            (TaskStatus::Review, TaskStatus::Blocked),
            (TaskStatus::Blocked, TaskStatus::Review),
            (TaskStatus::Pending, TaskStatus::Review),
        ];

        for (from, to) in illegal {
            assert!(validate_transition(from, to, None).is_err());
        }
    }

    #[test]
    fn validate_transition_requires_outcome_for_done() {
        let err = validate_transition(TaskStatus::InProgress, TaskStatus::Done, None).unwrap_err();
        assert!(matches!(err, LifecycleError::MissingOutcome));
    }

    #[tokio::test]
    async fn apply_transition_loads_validates_and_persists() {
        let pool = test_pool().await;

        let session_id = Uuid::new_v4();
        let task = sample_task(session_id, "t1", TaskStatus::Pending, vec![]);
        db::insert_task(&pool, &task).await.unwrap();

        apply_transition(&pool, task.id, TaskStatus::InProgress, None)
            .await
            .unwrap();

        let updated = db::get_task(&pool, task.id).await.unwrap().unwrap();
        assert_eq!(updated.status, TaskStatus::InProgress);
    }

    #[tokio::test]
    async fn cascade_unblock_moves_blocked_dependents_to_pending_when_ready() {
        let pool = test_pool().await;

        let session_id = Uuid::new_v4();
        let dep1 = sample_task(session_id, "dep1", TaskStatus::Done, vec![]);
        let dep2 = sample_task(session_id, "dep2", TaskStatus::Done, vec![]);
        let blocked = sample_task(
            session_id,
            "blocked",
            TaskStatus::Blocked,
            vec![dep1.id, dep2.id],
        );
        let in_review = sample_task(
            session_id,
            "review",
            TaskStatus::Review,
            vec![dep1.id, dep2.id],
        );

        db::insert_task(&pool, &dep1).await.unwrap();
        db::insert_task(&pool, &dep2).await.unwrap();
        db::insert_task(&pool, &blocked).await.unwrap();
        db::insert_task(&pool, &in_review).await.unwrap();

        let unblocked = cascade_unblock(&pool, dep1.id).await.unwrap();
        assert_eq!(unblocked, vec![blocked.id]);

        let blocked_now = db::get_task(&pool, blocked.id).await.unwrap().unwrap();
        assert_eq!(blocked_now.status, TaskStatus::Pending);

        let review_now = db::get_task(&pool, in_review.id).await.unwrap().unwrap();
        assert_eq!(review_now.status, TaskStatus::Review);
    }

    #[tokio::test]
    async fn cascade_unblock_keeps_task_blocked_when_other_dependencies_not_done() {
        let pool = test_pool().await;

        let session_id = Uuid::new_v4();
        let dep1 = sample_task(session_id, "dep1", TaskStatus::Done, vec![]);
        let dep2 = sample_task(session_id, "dep2", TaskStatus::InProgress, vec![]);
        let blocked = sample_task(
            session_id,
            "blocked",
            TaskStatus::Blocked,
            vec![dep1.id, dep2.id],
        );

        db::insert_task(&pool, &dep1).await.unwrap();
        db::insert_task(&pool, &dep2).await.unwrap();
        db::insert_task(&pool, &blocked).await.unwrap();

        let unblocked = cascade_unblock(&pool, dep1.id).await.unwrap();
        assert!(unblocked.is_empty());

        let blocked_now = db::get_task(&pool, blocked.id).await.unwrap().unwrap();
        assert_eq!(blocked_now.status, TaskStatus::Blocked);
    }
}
