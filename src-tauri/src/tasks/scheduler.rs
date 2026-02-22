use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use uuid::Uuid;

use super::types::{Priority, Task, TaskOutcome, TaskStatus};

pub struct Scheduler {
    max_concurrent: usize,
    active_tasks: HashSet<Uuid>,
}

impl Scheduler {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            max_concurrent,
            active_tasks: HashSet::new(),
        }
    }

    /// Given the current state of tasks, return which tasks should be dispatched next.
    /// This is a pure function that reads from DB and returns task IDs to start.
    pub fn select_next(
        &self,
        conn: &Connection,
        session_id: Uuid,
    ) -> Result<Vec<Task>, SchedulerError> {
        let candidates = super::db::next_unblocked(conn, session_id)?;
        let available_slots = self.max_concurrent.saturating_sub(self.active_tasks.len());
        if available_slots == 0 {
            return Ok(Vec::new());
        }

        let mut heap = BinaryHeap::new();
        let mut tasks_by_id = HashMap::with_capacity(candidates.len());

        for task in candidates {
            if self.active_tasks.contains(&task.id) {
                continue;
            }
            heap.push(PrioritizedTask {
                priority: task.priority,
                created_at: task.created_at,
                task_id: task.id,
            });
            tasks_by_id.insert(task.id, task);
        }

        let mut selected = Vec::with_capacity(available_slots.min(tasks_by_id.len()));
        while selected.len() < available_slots {
            let Some(prioritized) = heap.pop() else {
                break;
            };
            if let Some(task) = tasks_by_id.get(&prioritized.task_id) {
                selected.push(task.clone());
            }
        }

        Ok(selected)
    }

    /// Run one scheduling cycle: pick tasks, transition them to InProgress, return them.
    pub fn dispatch_cycle(
        &mut self,
        conn: &Connection,
        session_id: Uuid,
    ) -> Result<Vec<Task>, SchedulerError> {
        let tasks_to_start = self.select_next(conn, session_id)?;

        for task in &tasks_to_start {
            super::lifecycle::apply_transition(conn, task.id, TaskStatus::InProgress, None)
                .map_err(map_lifecycle_err)?;
            self.mark_active(task.id);
        }

        Ok(tasks_to_start)
    }

    /// Called when a task finishes (success, failure, or block).
    /// Removes from active set and triggers cascade unblock for successful completion.
    pub fn on_task_complete(
        &mut self,
        conn: &Connection,
        task_id: Uuid,
        status: TaskStatus,
        outcome: Option<&TaskOutcome>,
    ) -> Result<Vec<Uuid>, SchedulerError> {
        self.mark_inactive(task_id);

        super::lifecycle::apply_transition(conn, task_id, status, outcome)
            .map_err(map_lifecycle_err)?;

        if status == TaskStatus::Done {
            super::lifecycle::cascade_unblock(conn, task_id).map_err(map_lifecycle_err)
        } else {
            Ok(Vec::new())
        }
    }

    /// Mark a task as actively being worked on.
    pub fn mark_active(&mut self, task_id: Uuid) {
        self.active_tasks.insert(task_id);
    }

    /// Mark a task as no longer active (completed, failed, blocked).
    pub fn mark_inactive(&mut self, task_id: Uuid) {
        self.active_tasks.remove(&task_id);
    }

    /// Get count of currently active tasks.
    pub fn active_count(&self) -> usize {
        self.active_tasks.len()
    }

    /// Check if a specific task is currently active.
    pub fn is_active(&self, task_id: Uuid) -> bool {
        self.active_tasks.contains(&task_id)
    }
}

/// Validate a task dependency graph is acyclic using Kahn's algorithm.
/// `edges` are `(task_id, depends_on_task_id)`.
pub fn validate_dag(tasks: &[Task], edges: &[(Uuid, Uuid)]) -> Result<Vec<Uuid>, SchedulerError> {
    let mut in_degree: HashMap<Uuid, usize> = tasks.iter().map(|task| (task.id, 0)).collect();
    let mut adjacency: HashMap<Uuid, Vec<Uuid>> = HashMap::new();

    for (task_id, depends_on_task_id) in edges {
        if !in_degree.contains_key(task_id) || !in_degree.contains_key(depends_on_task_id) {
            continue;
        }

        *in_degree
            .get_mut(task_id)
            .expect("task exists in in-degree map") += 1;
        adjacency
            .entry(*depends_on_task_id)
            .or_default()
            .push(*task_id);
    }

    let mut queue = VecDeque::new();
    for task in tasks {
        if in_degree.get(&task.id).copied().unwrap_or(0) == 0 {
            queue.push_back(task.id);
        }
    }

    let mut ordered = Vec::with_capacity(tasks.len());
    while let Some(task_id) = queue.pop_front() {
        ordered.push(task_id);

        if let Some(dependents) = adjacency.get(&task_id) {
            for dependent in dependents {
                if let Some(degree) = in_degree.get_mut(dependent) {
                    *degree = degree.saturating_sub(1);
                    if *degree == 0 {
                        queue.push_back(*dependent);
                    }
                }
            }
        }
    }

    if ordered.len() != tasks.len() {
        let cycle_nodes = tasks
            .iter()
            .filter(|task| in_degree.get(&task.id).copied().unwrap_or(0) > 0)
            .map(|task| task.id)
            .collect();
        return Err(SchedulerError::CycleDetected(cycle_nodes));
    }

    Ok(ordered)
}

#[derive(Debug, Eq, PartialEq)]
struct PrioritizedTask {
    priority: Priority,
    created_at: DateTime<Utc>,
    task_id: Uuid,
}

impl Ord for PrioritizedTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher priority first, then earlier created_at first (FIFO within same priority).
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.created_at.cmp(&self.created_at))
            .then_with(|| self.task_id.cmp(&other.task_id))
    }
}

impl PartialOrd for PrioritizedTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn map_lifecycle_err(err: super::lifecycle::LifecycleError) -> SchedulerError {
    match err {
        super::lifecycle::LifecycleError::DbError(db_err) => SchedulerError::DbError(db_err),
        other => SchedulerError::DbError(rusqlite::Error::InvalidParameterName(other.to_string())),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("Dependency cycle detected involving tasks: {0:?}")]
    CycleDetected(Vec<Uuid>),
    #[error("Database error: {0}")]
    DbError(#[from] rusqlite::Error),
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use chrono::{Duration, Utc};
    use rusqlite::Connection;
    use uuid::Uuid;

    use super::*;
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
        priority: Priority,
        created_at: DateTime<Utc>,
        depends_on: Vec<Uuid>,
    ) -> Task {
        Task {
            id: Uuid::new_v4(),
            session_id,
            title: title.to_string(),
            description: String::new(),
            status,
            priority,
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
            created_at,
            updated_at: created_at,
        }
    }

    #[test]
    fn validate_dag_returns_topological_order_for_valid_graph() {
        let session_id = Uuid::new_v4();
        let now = Utc::now();

        let a = sample_task(
            session_id,
            "a",
            TaskStatus::Pending,
            Priority::Low,
            now,
            vec![],
        );
        let b = sample_task(
            session_id,
            "b",
            TaskStatus::Pending,
            Priority::Low,
            now + Duration::seconds(1),
            vec![a.id],
        );
        let c = sample_task(
            session_id,
            "c",
            TaskStatus::Pending,
            Priority::Low,
            now + Duration::seconds(2),
            vec![b.id],
        );

        let tasks = vec![a.clone(), b.clone(), c.clone()];
        let edges = vec![(b.id, a.id), (c.id, b.id)];
        let ordered = validate_dag(&tasks, &edges).unwrap();
        assert_eq!(ordered, vec![a.id, b.id, c.id]);
    }

    #[test]
    fn validate_dag_detects_cycle() {
        let session_id = Uuid::new_v4();
        let now = Utc::now();

        let a = sample_task(
            session_id,
            "a",
            TaskStatus::Pending,
            Priority::Low,
            now,
            vec![],
        );
        let b = sample_task(
            session_id,
            "b",
            TaskStatus::Pending,
            Priority::Low,
            now + Duration::seconds(1),
            vec![],
        );
        let c = sample_task(
            session_id,
            "c",
            TaskStatus::Pending,
            Priority::Low,
            now + Duration::seconds(2),
            vec![],
        );

        let tasks = vec![a.clone(), b.clone(), c.clone()];
        let edges = vec![(a.id, c.id), (b.id, a.id), (c.id, b.id)];

        let err = validate_dag(&tasks, &edges).unwrap_err();
        match err {
            SchedulerError::CycleDetected(cycle_nodes) => {
                let expected = HashSet::from([a.id, b.id, c.id]);
                let actual: HashSet<_> = cycle_nodes.into_iter().collect();
                assert_eq!(actual, expected);
            }
            other => panic!("expected cycle error, got {other:?}"),
        }
    }

    #[test]
    fn select_next_respects_priority_fifo_and_max_concurrent() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn).unwrap();

        let session_id = Uuid::new_v4();
        let base = Utc::now();

        let high_new = sample_task(
            session_id,
            "high_new",
            TaskStatus::Pending,
            Priority::High,
            base + Duration::seconds(3),
            vec![],
        );
        let high_old = sample_task(
            session_id,
            "high_old",
            TaskStatus::Pending,
            Priority::High,
            base + Duration::seconds(1),
            vec![],
        );
        let critical = sample_task(
            session_id,
            "critical",
            TaskStatus::Pending,
            Priority::Critical,
            base + Duration::seconds(2),
            vec![],
        );
        let medium = sample_task(
            session_id,
            "medium",
            TaskStatus::Pending,
            Priority::Medium,
            base,
            vec![],
        );

        db::insert_task(&conn, &high_new).unwrap();
        db::insert_task(&conn, &high_old).unwrap();
        db::insert_task(&conn, &critical).unwrap();
        db::insert_task(&conn, &medium).unwrap();

        let scheduler = Scheduler::new(2);
        let selected = scheduler.select_next(&conn, session_id).unwrap();
        let selected_ids: Vec<_> = selected.iter().map(|task| task.id).collect();

        assert_eq!(selected_ids, vec![critical.id, high_old.id]);
    }

    #[test]
    fn select_next_returns_empty_when_at_capacity() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn).unwrap();

        let session_id = Uuid::new_v4();
        let task = sample_task(
            session_id,
            "pending",
            TaskStatus::Pending,
            Priority::Critical,
            Utc::now(),
            vec![],
        );
        db::insert_task(&conn, &task).unwrap();

        let mut scheduler = Scheduler::new(1);
        scheduler.mark_active(Uuid::new_v4());
        let selected = scheduler.select_next(&conn, session_id).unwrap();
        assert!(selected.is_empty());
    }

    #[test]
    fn dispatch_cycle_moves_tasks_to_in_progress_and_marks_active() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn).unwrap();

        let session_id = Uuid::new_v4();
        let base = Utc::now();
        let low = sample_task(
            session_id,
            "low",
            TaskStatus::Pending,
            Priority::Low,
            base,
            vec![],
        );
        let critical = sample_task(
            session_id,
            "critical",
            TaskStatus::Pending,
            Priority::Critical,
            base + Duration::seconds(1),
            vec![],
        );

        db::insert_task(&conn, &low).unwrap();
        db::insert_task(&conn, &critical).unwrap();

        let mut scheduler = Scheduler::new(1);
        let started = scheduler.dispatch_cycle(&conn, session_id).unwrap();

        assert_eq!(started.len(), 1);
        assert_eq!(started[0].id, critical.id);
        assert!(scheduler.is_active(critical.id));
        assert_eq!(scheduler.active_count(), 1);

        let updated = db::get_task(&conn, critical.id).unwrap().unwrap();
        assert_eq!(updated.status, TaskStatus::InProgress);
    }

    #[test]
    fn on_task_complete_marks_inactive_and_cascades_unblock_for_done() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn).unwrap();

        let session_id = Uuid::new_v4();
        let base = Utc::now();

        let dependency = sample_task(
            session_id,
            "dependency",
            TaskStatus::InProgress,
            Priority::High,
            base,
            vec![],
        );
        let blocked = sample_task(
            session_id,
            "blocked",
            TaskStatus::Blocked,
            Priority::Medium,
            base + Duration::seconds(1),
            vec![dependency.id],
        );

        db::insert_task(&conn, &dependency).unwrap();
        db::insert_task(&conn, &blocked).unwrap();

        let mut scheduler = Scheduler::new(2);
        scheduler.mark_active(dependency.id);
        assert!(scheduler.is_active(dependency.id));

        let outcome = done_outcome();
        let unblocked = scheduler
            .on_task_complete(&conn, dependency.id, TaskStatus::Done, Some(&outcome))
            .unwrap();

        assert!(!scheduler.is_active(dependency.id));
        assert_eq!(unblocked, vec![blocked.id]);

        let done_task = db::get_task(&conn, dependency.id).unwrap().unwrap();
        assert_eq!(done_task.status, TaskStatus::Done);

        let unblocked_task = db::get_task(&conn, blocked.id).unwrap().unwrap();
        assert_eq!(unblocked_task.status, TaskStatus::Pending);
    }
}
