use rusqlite::{params, Connection};
use uuid::Uuid;

/// Cost thresholds from configuration.
/// These are passed in rather than read from config directly.
#[derive(Debug, Clone, Copy)]
pub struct CostThresholds {
    pub warn_at_task_usd: f64,
    pub hard_limit_task_usd: f64,
    pub warn_at_session_usd: f64,
    pub hard_limit_session_usd: f64,
}

impl Default for CostThresholds {
    fn default() -> Self {
        Self {
            warn_at_task_usd: 2.0,
            hard_limit_task_usd: 5.0,
            warn_at_session_usd: 20.0,
            hard_limit_session_usd: 50.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CostCheckResult {
    /// Under all thresholds.
    Ok,
    /// Task cost exceeds warning threshold.
    TaskWarning {
        task_id: Uuid,
        cost_usd: f64,
        threshold: f64,
    },
    /// Session cost exceeds warning threshold.
    SessionWarning {
        session_id: Uuid,
        cost_usd: f64,
        threshold: f64,
    },
    /// Task cost exceeds hard limit; halt this task's agent.
    TaskHardLimit {
        task_id: Uuid,
        cost_usd: f64,
        limit: f64,
    },
    /// Session cost exceeds hard limit; halt all agents.
    SessionHardLimit {
        session_id: Uuid,
        cost_usd: f64,
        limit: f64,
    },
}

/// Add cost to a task and return the new total cost for that task.
pub fn record_task_cost(
    conn: &Connection,
    task_id: Uuid,
    cost_increment_usd: f64,
) -> rusqlite::Result<f64> {
    conn.execute(
        "UPDATE tasks SET cost_usd = cost_usd + ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
        params![cost_increment_usd, task_id.to_string()],
    )?;

    conn.query_row(
        "SELECT cost_usd FROM tasks WHERE id = ?1",
        params![task_id.to_string()],
        |row| row.get(0),
    )
}

/// Get total cost for all tasks in a session.
pub fn session_cost(conn: &Connection, session_id: Uuid) -> rusqlite::Result<f64> {
    conn.query_row(
        "SELECT COALESCE(SUM(cost_usd), 0.0) FROM tasks WHERE session_id = ?1",
        params![session_id.to_string()],
        |row| row.get(0),
    )
}

/// Check thresholds for a specific task and its session.
/// Returns the most severe violation found: hard limit > warning > ok.
pub fn check_cost(
    conn: &Connection,
    task_id: Uuid,
    session_id: Uuid,
    thresholds: &CostThresholds,
) -> rusqlite::Result<CostCheckResult> {
    let task_cost: f64 = conn.query_row(
        "SELECT cost_usd FROM tasks WHERE id = ?1",
        params![task_id.to_string()],
        |row| row.get(0),
    )?;

    // Hard limits are enforced immediately (>=) with no grace period.
    if task_cost >= thresholds.hard_limit_task_usd {
        return Ok(CostCheckResult::TaskHardLimit {
            task_id,
            cost_usd: task_cost,
            limit: thresholds.hard_limit_task_usd,
        });
    }

    let session_cost_total = session_cost(conn, session_id)?;

    if session_cost_total >= thresholds.hard_limit_session_usd {
        return Ok(CostCheckResult::SessionHardLimit {
            session_id,
            cost_usd: session_cost_total,
            limit: thresholds.hard_limit_session_usd,
        });
    }

    if task_cost >= thresholds.warn_at_task_usd {
        return Ok(CostCheckResult::TaskWarning {
            task_id,
            cost_usd: task_cost,
            threshold: thresholds.warn_at_task_usd,
        });
    }

    if session_cost_total >= thresholds.warn_at_session_usd {
        return Ok(CostCheckResult::SessionWarning {
            session_id,
            cost_usd: session_cost_total,
            threshold: thresholds.warn_at_session_usd,
        });
    }

    Ok(CostCheckResult::Ok)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CostAction {
    /// No action needed.
    Continue,
    /// Show warning to the user but continue.
    Warn(String),
    /// Halt only this task's agent.
    HaltTask(Uuid),
    /// Halt all agents in this session.
    HaltAll(Uuid),
}

pub fn enforcement_action(check: &CostCheckResult) -> CostAction {
    match check {
        CostCheckResult::Ok => CostAction::Continue,
        CostCheckResult::TaskWarning {
            cost_usd,
            threshold,
            ..
        } => CostAction::Warn(format!(
            "Task cost ${cost_usd:.2} exceeds warning threshold ${threshold:.2}"
        )),
        CostCheckResult::SessionWarning {
            cost_usd,
            threshold,
            ..
        } => CostAction::Warn(format!(
            "Session cost ${cost_usd:.2} exceeds warning threshold ${threshold:.2}"
        )),
        CostCheckResult::TaskHardLimit { task_id, .. } => CostAction::HaltTask(*task_id),
        CostCheckResult::SessionHardLimit { session_id, .. } => CostAction::HaltAll(*session_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_task_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::tasks::db::init_schema(&conn).unwrap();
        conn
    }

    fn insert_task(conn: &Connection, task_id: Uuid, session_id: Uuid, cost_usd: f64) {
        conn.execute(
            "INSERT INTO tasks (id, session_id, title, cost_usd) VALUES (?1, ?2, ?3, ?4)",
            params![
                task_id.to_string(),
                session_id.to_string(),
                format!("task-{task_id}"),
                cost_usd
            ],
        )
        .unwrap();
    }

    #[test]
    fn default_thresholds_match_spec() {
        let defaults = CostThresholds::default();
        assert_eq!(defaults.warn_at_task_usd, 2.0);
        assert_eq!(defaults.hard_limit_task_usd, 5.0);
        assert_eq!(defaults.warn_at_session_usd, 20.0);
        assert_eq!(defaults.hard_limit_session_usd, 50.0);
    }

    #[test]
    fn record_task_cost_increments_and_returns_total() {
        let conn = open_task_db();
        let session_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        insert_task(&conn, task_id, session_id, 1.25);

        let total = record_task_cost(&conn, task_id, 0.75).unwrap();
        assert!((total - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn session_cost_sums_all_tasks_in_session() {
        let conn = open_task_db();
        let target_session = Uuid::new_v4();
        let other_session = Uuid::new_v4();

        insert_task(&conn, Uuid::new_v4(), target_session, 1.10);
        insert_task(&conn, Uuid::new_v4(), target_session, 2.40);
        insert_task(&conn, Uuid::new_v4(), other_session, 9.99);

        let total = session_cost(&conn, target_session).unwrap();
        assert!((total - 3.50).abs() < f64::EPSILON);
    }

    #[test]
    fn check_cost_prioritizes_task_hard_limit() {
        let conn = open_task_db();
        let session_id = Uuid::new_v4();
        let target_task = Uuid::new_v4();

        insert_task(&conn, target_task, session_id, 5.0);
        insert_task(&conn, Uuid::new_v4(), session_id, 100.0);

        let thresholds = CostThresholds {
            warn_at_task_usd: 2.0,
            hard_limit_task_usd: 5.0,
            warn_at_session_usd: 20.0,
            hard_limit_session_usd: 50.0,
        };

        let result = check_cost(&conn, target_task, session_id, &thresholds).unwrap();
        assert_eq!(
            result,
            CostCheckResult::TaskHardLimit {
                task_id: target_task,
                cost_usd: 5.0,
                limit: 5.0,
            }
        );
    }

    #[test]
    fn check_cost_prioritizes_session_hard_limit_over_warnings() {
        let conn = open_task_db();
        let session_id = Uuid::new_v4();
        let target_task = Uuid::new_v4();

        insert_task(&conn, target_task, session_id, 3.0);
        insert_task(&conn, Uuid::new_v4(), session_id, 8.0);

        let thresholds = CostThresholds {
            warn_at_task_usd: 2.0,
            hard_limit_task_usd: 5.0,
            warn_at_session_usd: 10.0,
            hard_limit_session_usd: 11.0,
        };

        let result = check_cost(&conn, target_task, session_id, &thresholds).unwrap();
        assert_eq!(
            result,
            CostCheckResult::SessionHardLimit {
                session_id,
                cost_usd: 11.0,
                limit: 11.0,
            }
        );
    }

    #[test]
    fn check_cost_returns_warning_when_limits_not_hit() {
        let conn = open_task_db();
        let session_id = Uuid::new_v4();
        let target_task = Uuid::new_v4();

        insert_task(&conn, target_task, session_id, 2.5);
        insert_task(&conn, Uuid::new_v4(), session_id, 1.0);

        let thresholds = CostThresholds {
            warn_at_task_usd: 2.0,
            hard_limit_task_usd: 5.0,
            warn_at_session_usd: 20.0,
            hard_limit_session_usd: 50.0,
        };

        let result = check_cost(&conn, target_task, session_id, &thresholds).unwrap();
        assert_eq!(
            result,
            CostCheckResult::TaskWarning {
                task_id: target_task,
                cost_usd: 2.5,
                threshold: 2.0,
            }
        );
    }

    #[test]
    fn enforcement_action_maps_results() {
        let task_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();

        assert_eq!(
            enforcement_action(&CostCheckResult::Ok),
            CostAction::Continue
        );

        assert_eq!(
            enforcement_action(&CostCheckResult::TaskWarning {
                task_id,
                cost_usd: 2.5,
                threshold: 2.0,
            }),
            CostAction::Warn("Task cost $2.50 exceeds warning threshold $2.00".to_string())
        );

        assert_eq!(
            enforcement_action(&CostCheckResult::SessionWarning {
                session_id,
                cost_usd: 20.0,
                threshold: 10.0,
            }),
            CostAction::Warn("Session cost $20.00 exceeds warning threshold $10.00".to_string())
        );

        assert_eq!(
            enforcement_action(&CostCheckResult::TaskHardLimit {
                task_id,
                cost_usd: 5.0,
                limit: 5.0,
            }),
            CostAction::HaltTask(task_id)
        );

        assert_eq!(
            enforcement_action(&CostCheckResult::SessionHardLimit {
                session_id,
                cost_usd: 50.0,
                limit: 50.0,
            }),
            CostAction::HaltAll(session_id)
        );
    }
}
