use std::collections::HashSet;

use chrono::Utc;
use rusqlite::Connection;
use uuid::Uuid;

use super::costs::{self, CostAction, CostCheckResult, CostThresholds};
use super::decomposition::{
    self, DecompositionError, DecompositionRequest, DecompositionResult, PlannedTask,
};
use super::engagement;
use super::lifecycle::{self, LifecycleError};
use super::scheduler::{self, Scheduler, SchedulerError};
use super::*;

fn setup_db() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory db should open");
    db::init_schema(&conn).expect("task schema should initialize");
    conn
}

fn make_task(title: &str, session_id: Uuid) -> Task {
    let now = Utc::now();
    Task {
        id: Uuid::new_v4(),
        session_id,
        title: title.to_string(),
        description: format!("Description for {title}"),
        status: TaskStatus::Pending,
        priority: Priority::Medium,
        assigned_agent: None,
        parent_task: None,
        depends_on: vec![],
        worktree_branch: None,
        base_ref: "main".to_string(),
        base_commit: "abc123".to_string(),
        merge_target_ref: "main".to_string(),
        outcome: None,
        engagement_override: None,
        cost_usd: 0.0,
        created_at: now,
        updated_at: now,
    }
}

fn success_outcome() -> TaskOutcome {
    TaskOutcome::Success {
        summary: "done".to_string(),
        diff_stat: DiffStat {
            files_changed: 1,
            insertions: 10,
            deletions: 2,
        },
    }
}

mod type_tests {
    use super::*;

    #[test]
    fn priority_ordering() {
        assert!(Priority::Low < Priority::Medium);
        assert!(Priority::Medium < Priority::High);
        assert!(Priority::High < Priority::Critical);
    }

    #[test]
    fn task_outcome_serialization_roundtrip() {
        let success = TaskOutcome::Success {
            summary: "done".to_string(),
            diff_stat: DiffStat {
                files_changed: 3,
                insertions: 100,
                deletions: 20,
            },
        };
        let success_json = serde_json::to_string(&success).unwrap();
        assert!(success_json.contains("\"kind\""));
        assert!(success_json.contains("\"data\""));
        let parsed_success: TaskOutcome = serde_json::from_str(&success_json).unwrap();
        match parsed_success {
            TaskOutcome::Success { summary, diff_stat } => {
                assert_eq!(summary, "done");
                assert_eq!(diff_stat.files_changed, 3);
                assert_eq!(diff_stat.insertions, 100);
                assert_eq!(diff_stat.deletions, 20);
            }
            other => panic!("expected success outcome, got {other:?}"),
        }

        let failure = TaskOutcome::Failure {
            reason: "cost limit".to_string(),
        };
        let failure_json = serde_json::to_string(&failure).unwrap();
        assert!(failure_json.contains("\"kind\""));
        let parsed_failure: TaskOutcome = serde_json::from_str(&failure_json).unwrap();
        match parsed_failure {
            TaskOutcome::Failure { reason } => assert_eq!(reason, "cost limit"),
            other => panic!("expected failure outcome, got {other:?}"),
        }

        let needs_human = TaskOutcome::NeedsHuman {
            question: "Need product decision".to_string(),
            context: "Two approaches are viable".to_string(),
        };
        let needs_human_json = serde_json::to_string(&needs_human).unwrap();
        assert!(needs_human_json.contains("\"kind\""));
        let parsed_needs_human: TaskOutcome = serde_json::from_str(&needs_human_json).unwrap();
        match parsed_needs_human {
            TaskOutcome::NeedsHuman { question, context } => {
                assert_eq!(question, "Need product decision");
                assert_eq!(context, "Two approaches are viable");
            }
            other => panic!("expected needs_human outcome, got {other:?}"),
        }
    }

    #[test]
    fn task_status_serde() {
        let json = serde_json::to_string(&TaskStatus::InProgress).unwrap();
        assert_eq!(json, "\"in_progress\"");
    }

    #[test]
    fn engagement_level_serde() {
        let json = serde_json::to_string(&EngagementLevel::ReviewGates).unwrap();
        assert_eq!(json, "\"review_gates\"");
    }
}

mod db_tests {
    use super::*;

    #[test]
    fn insert_and_get_task() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();
        let task = make_task("Test task", session_id);

        db::insert_task(&conn, &task).unwrap();
        let loaded = db::get_task(&conn, task.id).unwrap().unwrap();

        assert_eq!(loaded.title, "Test task");
        assert_eq!(loaded.status, TaskStatus::Pending);
        assert_eq!(loaded.depends_on, Vec::<Uuid>::new());
    }

    #[test]
    fn insert_task_with_dependencies() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();
        let dependency = make_task("Dependency", session_id);
        db::insert_task(&conn, &dependency).unwrap();

        let mut dependent = make_task("Dependent", session_id);
        dependent.depends_on = vec![dependency.id];
        db::insert_task(&conn, &dependent).unwrap();

        let loaded = db::get_task(&conn, dependent.id).unwrap().unwrap();
        assert_eq!(loaded.depends_on, vec![dependency.id]);
    }

    #[test]
    fn list_tasks_with_status_filter() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();

        let pending = make_task("Pending", session_id);
        db::insert_task(&conn, &pending).unwrap();

        let mut in_progress = make_task("In progress", session_id);
        in_progress.status = TaskStatus::InProgress;
        db::insert_task(&conn, &in_progress).unwrap();

        let filtered = db::list_tasks(&conn, session_id, Some(TaskStatus::Pending)).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, pending.id);
        assert_eq!(filtered[0].status, TaskStatus::Pending);
    }

    #[test]
    fn update_task_status_with_outcome() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();
        let task = make_task("Target", session_id);
        db::insert_task(&conn, &task).unwrap();

        let outcome = success_outcome();
        db::update_task_status(&conn, task.id, TaskStatus::Done, Some(&outcome)).unwrap();

        let loaded = db::get_task(&conn, task.id).unwrap().unwrap();
        assert_eq!(loaded.status, TaskStatus::Done);
        assert!(matches!(
            loaded.outcome,
            Some(TaskOutcome::Success { summary, .. }) if summary == "done"
        ));
    }

    #[test]
    fn next_unblocked_respects_dependencies() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();

        let dependency = make_task("A", session_id);
        db::insert_task(&conn, &dependency).unwrap();

        let mut dependent = make_task("B", session_id);
        dependent.depends_on = vec![dependency.id];
        db::insert_task(&conn, &dependent).unwrap();

        let initial = db::next_unblocked(&conn, session_id).unwrap();
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].id, dependency.id);

        let outcome = success_outcome();
        db::update_task_status(&conn, dependency.id, TaskStatus::Done, Some(&outcome)).unwrap();

        let after = db::next_unblocked(&conn, session_id).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, dependent.id);
    }

    #[test]
    fn next_unblocked_priority_ordering() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();

        let mut low = make_task("Low", session_id);
        low.priority = Priority::Low;
        db::insert_task(&conn, &low).unwrap();

        let mut critical = make_task("Critical", session_id);
        critical.priority = Priority::Critical;
        db::insert_task(&conn, &critical).unwrap();

        let unblocked = db::next_unblocked(&conn, session_id).unwrap();
        assert_eq!(unblocked[0].id, critical.id);
    }

    #[test]
    fn get_task_tree_returns_edges() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();

        let parent = make_task("Parent", session_id);
        db::insert_task(&conn, &parent).unwrap();

        let mut child = make_task("Child", session_id);
        child.depends_on = vec![parent.id];
        db::insert_task(&conn, &child).unwrap();

        let (tasks, edges) = db::get_task_tree(&conn, session_id).unwrap();

        assert_eq!(tasks.len(), 2);
        assert!(edges.contains(&(child.id, parent.id)));
    }

    #[test]
    fn get_task_returns_none_for_missing() {
        let conn = setup_db();
        let task = db::get_task(&conn, Uuid::new_v4()).unwrap();
        assert!(task.is_none());
    }
}

mod lifecycle_tests {
    use super::*;

    #[test]
    fn valid_transitions() {
        assert!(
            lifecycle::validate_transition(TaskStatus::Pending, TaskStatus::InProgress, None)
                .is_ok()
        );
        assert!(
            lifecycle::validate_transition(TaskStatus::InProgress, TaskStatus::Review, None)
                .is_ok()
        );
        let done = success_outcome();
        assert!(
            lifecycle::validate_transition(TaskStatus::Review, TaskStatus::Done, Some(&done))
                .is_ok()
        );
    }

    #[test]
    fn invalid_transitions() {
        assert!(
            lifecycle::validate_transition(TaskStatus::Done, TaskStatus::Pending, None).is_err()
        );
        assert!(
            lifecycle::validate_transition(TaskStatus::Review, TaskStatus::Blocked, None).is_err()
        );
    }

    #[test]
    fn done_requires_outcome() {
        let err =
            lifecycle::validate_transition(TaskStatus::InProgress, TaskStatus::Done, None)
                .unwrap_err();
        assert!(matches!(err, LifecycleError::MissingOutcome));
    }

    #[test]
    fn any_to_done_with_failure() {
        let failure = TaskOutcome::Failure {
            reason: "cost limit".to_string(),
        };
        for status in [
            TaskStatus::Pending,
            TaskStatus::InProgress,
            TaskStatus::Blocked,
            TaskStatus::Review,
        ] {
            assert!(
                lifecycle::validate_transition(status, TaskStatus::Done, Some(&failure)).is_ok()
            );
        }
    }

    #[test]
    fn cascade_unblock() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();

        let mut completed = make_task("A", session_id);
        completed.status = TaskStatus::Done;
        completed.outcome = Some(success_outcome());
        db::insert_task(&conn, &completed).unwrap();

        let mut blocked = make_task("B", session_id);
        blocked.status = TaskStatus::Blocked;
        blocked.depends_on = vec![completed.id];
        db::insert_task(&conn, &blocked).unwrap();

        let unblocked = lifecycle::cascade_unblock(&conn, completed.id).unwrap();
        assert_eq!(unblocked, vec![blocked.id]);

        let loaded = db::get_task(&conn, blocked.id).unwrap().unwrap();
        assert_eq!(loaded.status, TaskStatus::Pending);
    }

    #[test]
    fn cascade_unblock_partial_deps() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();

        let mut dep_done = make_task("A", session_id);
        dep_done.status = TaskStatus::Done;
        dep_done.outcome = Some(success_outcome());
        db::insert_task(&conn, &dep_done).unwrap();

        let dep_pending = make_task("C", session_id);
        db::insert_task(&conn, &dep_pending).unwrap();

        let mut blocked = make_task("B", session_id);
        blocked.status = TaskStatus::Blocked;
        blocked.depends_on = vec![dep_done.id, dep_pending.id];
        db::insert_task(&conn, &blocked).unwrap();

        let unblocked = lifecycle::cascade_unblock(&conn, dep_done.id).unwrap();
        assert!(unblocked.is_empty());

        let loaded = db::get_task(&conn, blocked.id).unwrap().unwrap();
        assert_eq!(loaded.status, TaskStatus::Blocked);
    }
}

mod scheduler_tests {
    use super::*;

    #[test]
    fn select_next_respects_max_concurrent() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();

        for idx in 0..5 {
            let task = make_task(&format!("Task {idx}"), session_id);
            db::insert_task(&conn, &task).unwrap();
        }

        let scheduler = Scheduler::new(2);
        let selected = scheduler.select_next(&conn, session_id).unwrap();
        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn select_next_returns_empty_at_capacity() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();
        let task = make_task("T", session_id);
        db::insert_task(&conn, &task).unwrap();

        let mut scheduler = Scheduler::new(1);
        scheduler.mark_active(Uuid::new_v4());
        let selected = scheduler.select_next(&conn, session_id).unwrap();
        assert!(selected.is_empty());
    }

    #[test]
    fn select_next_priority_ordering() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();

        let mut low = make_task("Low", session_id);
        low.priority = Priority::Low;
        db::insert_task(&conn, &low).unwrap();

        let mut medium = make_task("Medium", session_id);
        medium.priority = Priority::Medium;
        db::insert_task(&conn, &medium).unwrap();

        let mut high = make_task("High", session_id);
        high.priority = Priority::High;
        db::insert_task(&conn, &high).unwrap();

        let mut critical = make_task("Critical", session_id);
        critical.priority = Priority::Critical;
        db::insert_task(&conn, &critical).unwrap();

        let scheduler = Scheduler::new(4);
        let selected = scheduler.select_next(&conn, session_id).unwrap();
        assert_eq!(selected[0].id, critical.id);
    }

    #[test]
    fn validate_dag_detects_cycles() {
        let session_id = Uuid::new_v4();
        let a = make_task("A", session_id);
        let b = make_task("B", session_id);
        let c = make_task("C", session_id);
        let tasks = vec![a.clone(), b.clone(), c.clone()];
        let edges = vec![(a.id, c.id), (b.id, a.id), (c.id, b.id)];

        let err = scheduler::validate_dag(&tasks, &edges).unwrap_err();
        match err {
            SchedulerError::CycleDetected(nodes) => {
                let actual: HashSet<Uuid> = nodes.into_iter().collect();
                let expected = HashSet::from([a.id, b.id, c.id]);
                assert_eq!(actual, expected);
            }
            other => panic!("expected cycle error, got {other:?}"),
        }
    }

    #[test]
    fn validate_dag_valid_graph() {
        let session_id = Uuid::new_v4();
        let a = make_task("A", session_id);
        let b = make_task("B", session_id);
        let c = make_task("C", session_id);
        let tasks = vec![a.clone(), b.clone(), c.clone()];
        let edges = vec![(b.id, a.id), (c.id, b.id)];

        let ordered = scheduler::validate_dag(&tasks, &edges).unwrap();
        let a_idx = ordered.iter().position(|id| *id == a.id).unwrap();
        let b_idx = ordered.iter().position(|id| *id == b.id).unwrap();
        let c_idx = ordered.iter().position(|id| *id == c.id).unwrap();
        assert!(a_idx < b_idx && b_idx < c_idx);
    }

    #[test]
    fn dispatch_cycle_transitions_to_in_progress() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();

        let first = make_task("First", session_id);
        let second = make_task("Second", session_id);
        db::insert_task(&conn, &first).unwrap();
        db::insert_task(&conn, &second).unwrap();

        let mut scheduler = Scheduler::new(2);
        let started = scheduler.dispatch_cycle(&conn, session_id).unwrap();
        assert_eq!(started.len(), 2);

        for task in started {
            let loaded = db::get_task(&conn, task.id).unwrap().unwrap();
            assert_eq!(loaded.status, TaskStatus::InProgress);
        }
    }

    #[test]
    fn on_task_complete_unblocks_dependents() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();

        let mut dependency = make_task("Dependency", session_id);
        dependency.status = TaskStatus::InProgress;
        db::insert_task(&conn, &dependency).unwrap();

        let mut blocked = make_task("Blocked", session_id);
        blocked.status = TaskStatus::Blocked;
        blocked.depends_on = vec![dependency.id];
        db::insert_task(&conn, &blocked).unwrap();

        let mut scheduler = Scheduler::new(2);
        scheduler.mark_active(dependency.id);

        let outcome = success_outcome();
        let unblocked = scheduler
            .on_task_complete(&conn, dependency.id, TaskStatus::Done, Some(&outcome))
            .unwrap();
        assert_eq!(unblocked, vec![blocked.id]);

        let loaded = db::get_task(&conn, blocked.id).unwrap().unwrap();
        assert_eq!(loaded.status, TaskStatus::Pending);
    }
}

mod decomposition_tests {
    use super::*;

    #[test]
    fn validate_rejects_duplicate_titles() {
        let result = DecompositionResult {
            tasks: vec![
                PlannedTask {
                    title: "A".to_string(),
                    description: String::new(),
                    priority: Priority::Medium,
                    depends_on_titles: vec![],
                    parent_title: None,
                },
                PlannedTask {
                    title: "A".to_string(),
                    description: String::new(),
                    priority: Priority::Medium,
                    depends_on_titles: vec![],
                    parent_title: None,
                },
            ],
        };

        let err = decomposition::validate_decomposition(&result).unwrap_err();
        assert!(matches!(err, DecompositionError::DuplicateTitle(title) if title == "A"));
    }

    #[test]
    fn validate_rejects_unknown_dependency() {
        let result = DecompositionResult {
            tasks: vec![PlannedTask {
                title: "A".to_string(),
                description: String::new(),
                priority: Priority::Medium,
                depends_on_titles: vec!["B".to_string()],
                parent_title: None,
            }],
        };

        let err = decomposition::validate_decomposition(&result).unwrap_err();
        assert!(matches!(
            err,
            DecompositionError::UnknownDependency { task, dependency }
                if task == "A" && dependency == "B"
        ));
    }

    #[test]
    fn persist_decomposition_creates_tasks() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();
        let request = DecompositionRequest {
            session_id,
            project_description: "test".to_string(),
            base_ref: "main".to_string(),
            merge_target_ref: "main".to_string(),
        };
        let result = DecompositionResult {
            tasks: vec![
                PlannedTask {
                    title: "A".to_string(),
                    description: "do A".to_string(),
                    priority: Priority::High,
                    depends_on_titles: vec![],
                    parent_title: None,
                },
                PlannedTask {
                    title: "B".to_string(),
                    description: "do B".to_string(),
                    priority: Priority::Medium,
                    depends_on_titles: vec!["A".to_string()],
                    parent_title: None,
                },
            ],
        };

        let tasks = decomposition::persist_decomposition(&conn, &request, &result).unwrap();
        assert_eq!(tasks.len(), 2);

        let a = tasks.iter().find(|task| task.title == "A").unwrap();
        let b = tasks.iter().find(|task| task.title == "B").unwrap();
        assert_eq!(b.depends_on, vec![a.id]);
    }
}

mod engagement_tests {
    use super::*;

    #[test]
    fn resolve_with_override() {
        let mut task = make_task("T", Uuid::new_v4());
        task.engagement_override = Some(EngagementLevel::Collaborative);
        let resolved = engagement::resolve_engagement(&task, EngagementLevel::Autonomous);
        assert_eq!(resolved, EngagementLevel::Collaborative);
    }

    #[test]
    fn resolve_without_override_uses_default() {
        let task = make_task("T", Uuid::new_v4());
        let resolved = engagement::resolve_engagement(&task, EngagementLevel::ReviewGates);
        assert_eq!(resolved, EngagementLevel::ReviewGates);
    }

    #[test]
    fn autonomous_never_gates() {
        assert!(!engagement::needs_gate_check(
            EngagementLevel::Autonomous,
            TaskStatus::Review
        ));
        assert!(!engagement::needs_gate_check(
            EngagementLevel::Autonomous,
            TaskStatus::InProgress
        ));
    }

    #[test]
    fn review_gates_at_review() {
        assert!(engagement::needs_gate_check(
            EngagementLevel::ReviewGates,
            TaskStatus::Review
        ));
        assert!(!engagement::needs_gate_check(
            EngagementLevel::ReviewGates,
            TaskStatus::InProgress
        ));
    }

    #[test]
    fn collaborative_gates_at_in_progress_and_review() {
        assert!(engagement::needs_gate_check(
            EngagementLevel::Collaborative,
            TaskStatus::InProgress
        ));
        assert!(engagement::needs_gate_check(
            EngagementLevel::Collaborative,
            TaskStatus::Review
        ));
    }

    #[test]
    fn collaborative_forces_single_concurrency() {
        assert_eq!(
            engagement::effective_max_concurrent(EngagementLevel::Collaborative, 4),
            1
        );
        assert_eq!(
            engagement::effective_max_concurrent(EngagementLevel::Autonomous, 4),
            4
        );
    }
}

mod cost_tests {
    use super::*;

    #[test]
    fn record_and_read_cost() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();
        let task = make_task("T", session_id);
        db::insert_task(&conn, &task).unwrap();

        let total = costs::record_task_cost(&conn, task.id, 1.5).unwrap();
        assert!((total - 1.5).abs() < f64::EPSILON);

        let total = costs::record_task_cost(&conn, task.id, 0.5).unwrap();
        assert!((total - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn session_cost_sums_all_tasks() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();
        let other_session = Uuid::new_v4();

        let t1 = make_task("T1", session_id);
        let t2 = make_task("T2", session_id);
        let t3 = make_task("T3", other_session);
        db::insert_task(&conn, &t1).unwrap();
        db::insert_task(&conn, &t2).unwrap();
        db::insert_task(&conn, &t3).unwrap();

        costs::record_task_cost(&conn, t1.id, 1.0).unwrap();
        costs::record_task_cost(&conn, t2.id, 2.0).unwrap();
        costs::record_task_cost(&conn, t3.id, 9.0).unwrap();

        let total = costs::session_cost(&conn, session_id).unwrap();
        assert!((total - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn check_cost_task_hard_limit() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();
        let task = make_task("T", session_id);
        db::insert_task(&conn, &task).unwrap();

        costs::record_task_cost(&conn, task.id, 6.0).unwrap();
        let thresholds = CostThresholds::default();
        let result = costs::check_cost(&conn, task.id, session_id, &thresholds).unwrap();

        assert!(matches!(
            result,
            CostCheckResult::TaskHardLimit {
                task_id,
                cost_usd,
                limit
            } if task_id == task.id && (cost_usd - 6.0).abs() < f64::EPSILON && (limit - 5.0).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn check_cost_session_warning() {
        let conn = setup_db();
        let session_id = Uuid::new_v4();
        let task = make_task("T", session_id);
        let other = make_task("Other", session_id);
        db::insert_task(&conn, &task).unwrap();
        db::insert_task(&conn, &other).unwrap();

        costs::record_task_cost(&conn, task.id, 1.0).unwrap();
        costs::record_task_cost(&conn, other.id, 4.5).unwrap();

        let thresholds = CostThresholds {
            warn_at_task_usd: 10.0,
            hard_limit_task_usd: 20.0,
            warn_at_session_usd: 5.0,
            hard_limit_session_usd: 100.0,
        };
        let result = costs::check_cost(&conn, task.id, session_id, &thresholds).unwrap();

        assert!(matches!(
            result,
            CostCheckResult::SessionWarning {
                session_id: id,
                cost_usd,
                threshold
            } if id == session_id && (cost_usd - 5.5).abs() < f64::EPSILON && (threshold - 5.0).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn enforcement_action_mapping() {
        assert_eq!(
            costs::enforcement_action(&CostCheckResult::Ok),
            CostAction::Continue
        );

        let task_id = Uuid::new_v4();
        let halt = costs::enforcement_action(&CostCheckResult::TaskHardLimit {
            task_id,
            cost_usd: 6.0,
            limit: 5.0,
        });
        assert!(matches!(halt, CostAction::HaltTask(id) if id == task_id));
    }
}
