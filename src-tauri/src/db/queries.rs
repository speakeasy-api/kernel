use std::str::FromStr;

use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use super::models::{
    Agent, Event, Mode, Priority, Recommendation, Session, StatsRollup, Task, TaskStatus,
    UxAgentState,
};

// ---- Helpers ----

fn invalid_data(msg: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, msg)),
    )
}

// ---- Row mappers ----

fn row_to_session(row: &rusqlite::Row) -> Result<Session, rusqlite::Error> {
    Ok(Session {
        id: row.get(0)?,
        project_path: row.get(1)?,
        created_at: row.get(2)?,
    })
}

fn row_to_event(row: &rusqlite::Row) -> Result<Event, rusqlite::Error> {
    Ok(Event {
        id: row.get(0)?,
        kind: row.get(1)?,
        session_id: row.get(2)?,
        agent_id: row.get(3)?,
        data: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn row_to_task(row: &rusqlite::Row) -> Result<Task, rusqlite::Error> {
    let status_str: String = row.get(5)?;
    let priority_int: i32 = row.get(6)?;
    Ok(Task {
        id: row.get(0)?,
        session_id: row.get(1)?,
        parent_id: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        status: TaskStatus::from_str(&status_str).map_err(|e| invalid_data(e))?,
        priority: Priority::from_i32(priority_int).map_err(|e| invalid_data(e))?,
        worktree_branch: row.get(7)?,
        base_ref: row.get(8)?,
        base_commit: row.get(9)?,
        merge_target_ref: row.get(10)?,
        outcome_kind: row.get(11)?,
        outcome_data: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        completed_at: row.get(15)?,
    })
}

fn row_to_agent(row: &rusqlite::Row) -> Result<Agent, rusqlite::Error> {
    Ok(Agent {
        id: row.get(0)?,
        session_id: row.get(1)?,
        parent_agent_id: row.get(2)?,
        task_id: row.get(3)?,
        role: row.get(4)?,
        model: row.get(5)?,
        mode: row.get(6)?,
        status: row.get(7)?,
        token_input: row.get(8)?,
        token_output: row.get(9)?,
        created_at: row.get(10)?,
        finished_at: row.get(11)?,
    })
}

fn row_to_mode(row: &rusqlite::Row) -> Result<Mode, rusqlite::Error> {
    Ok(Mode {
        name: row.get(0)?,
        description: row.get(1)?,
        system_prompt: row.get(2)?,
        default_model: row.get(3)?,
        allowed_tools: row.get(4)?,
        origin: row.get(5)?,
        version: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn row_to_recommendation(row: &rusqlite::Row) -> Result<Recommendation, rusqlite::Error> {
    Ok(Recommendation {
        id: row.get(0)?,
        trigger_pattern: row.get(1)?,
        recommendation: row.get(2)?,
        action_type: row.get(3)?,
        action_payload: row.get(4)?,
        status: row.get(5)?,
        applied_at: row.get(6)?,
        reverted_at: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn row_to_stats_rollup(row: &rusqlite::Row) -> Result<StatsRollup, rusqlite::Error> {
    Ok(StatsRollup {
        id: row.get(0)?,
        period_start: row.get(1)?,
        period_end: row.get(2)?,
        scope: row.get(3)?,
        scope_id: row.get(4)?,
        metric: row.get(5)?,
        value: row.get(6)?,
    })
}

fn row_to_ux_agent_state(row: &rusqlite::Row) -> Result<UxAgentState, rusqlite::Error> {
    Ok(UxAgentState {
        scope: row.get(0)?,
        last_event_id: row.get(1)?,
        last_event_at: row.get(2)?,
        updated_at: row.get(3)?,
    })
}

// ---- Sessions ----

pub fn create_session(conn: &Connection, project_path: &str) -> Result<Session, rusqlite::Error> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO sessions (id, project_path) VALUES (?1, ?2)",
        params![id, project_path],
    )?;
    conn.query_row(
        "SELECT id, project_path, created_at FROM sessions WHERE id = ?1",
        params![id],
        row_to_session,
    )
}

pub fn get_session(conn: &Connection, id: &str) -> Result<Option<Session>, rusqlite::Error> {
    conn.query_row(
        "SELECT id, project_path, created_at FROM sessions WHERE id = ?1",
        params![id],
        row_to_session,
    )
    .optional()
}

// ---- Events ----

pub fn insert_event(
    conn: &Connection,
    session_id: &str,
    agent_id: Option<&str>,
    kind: &str,
    data: &str,
) -> Result<Event, rusqlite::Error> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO events (id, session_id, agent_id, kind, data) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, session_id, agent_id, kind, data],
    )?;
    conn.query_row(
        "SELECT id, kind, session_id, agent_id, data, created_at FROM events WHERE id = ?1",
        params![id],
        row_to_event,
    )
}

pub fn events_since(
    conn: &Connection,
    session_id: &str,
    since: &str,
) -> Result<Vec<Event>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, session_id, agent_id, data, created_at
         FROM events
         WHERE session_id = ?1 AND created_at > ?2
         ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(params![session_id, since], row_to_event)?;
    rows.collect()
}

pub fn events_by_kind(
    conn: &Connection,
    kind: &str,
    since: &str,
) -> Result<Vec<Event>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, session_id, agent_id, data, created_at
         FROM events
         WHERE kind = ?1 AND created_at > ?2
         ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(params![kind, since], row_to_event)?;
    rows.collect()
}

// ---- Tasks ----

const TASK_COLUMNS: &str = "id, session_id, parent_id, title, description, status, priority,
     worktree_branch, base_ref, base_commit, merge_target_ref,
     outcome_kind, outcome_data, created_at, updated_at, completed_at";

#[allow(clippy::too_many_arguments)]
pub fn create_task(
    conn: &Connection,
    session_id: &str,
    title: &str,
    description: Option<&str>,
    parent_id: Option<&str>,
    priority: &Priority,
    base_ref: &str,
    base_commit: &str,
    merge_target_ref: &str,
) -> Result<Task, rusqlite::Error> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO tasks (id, session_id, title, description, parent_id, priority,
                            base_ref, base_commit, merge_target_ref)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            session_id,
            title,
            description,
            parent_id,
            priority.as_i32(),
            base_ref,
            base_commit,
            merge_target_ref,
        ],
    )?;
    conn.query_row(
        &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
        params![id],
        row_to_task,
    )
}

pub fn get_task(conn: &Connection, id: &str) -> Result<Option<Task>, rusqlite::Error> {
    conn.query_row(
        &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
        params![id],
        row_to_task,
    )
    .optional()
}

pub fn list_tasks(
    conn: &Connection,
    session_id: &str,
    status_filter: Option<&TaskStatus>,
) -> Result<Vec<Task>, rusqlite::Error> {
    match status_filter {
        Some(status) => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM tasks
                 WHERE session_id = ?1 AND status = ?2
                 ORDER BY priority DESC, created_at ASC"
            ))?;
            let rows = stmt.query_map(params![session_id, status.to_string()], row_to_task)?;
            rows.collect()
        }
        None => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM tasks
                 WHERE session_id = ?1
                 ORDER BY priority DESC, created_at ASC"
            ))?;
            let rows = stmt.query_map(params![session_id], row_to_task)?;
            rows.collect()
        }
    }
}

pub fn update_task_status(
    conn: &Connection,
    id: &str,
    status: &TaskStatus,
    outcome_kind: Option<&str>,
    outcome_data: Option<&str>,
) -> Result<(), rusqlite::Error> {
    if *status == TaskStatus::Done {
        conn.execute(
            "UPDATE tasks
             SET status = ?1, outcome_kind = ?2, outcome_data = ?3,
                 updated_at = CURRENT_TIMESTAMP, completed_at = CURRENT_TIMESTAMP
             WHERE id = ?4",
            params![status.to_string(), outcome_kind, outcome_data, id],
        )?;
    } else {
        conn.execute(
            "UPDATE tasks
             SET status = ?1, outcome_kind = ?2, outcome_data = ?3,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?4",
            params![status.to_string(), outcome_kind, outcome_data, id],
        )?;
    }
    Ok(())
}

pub fn add_task_dep(
    conn: &Connection,
    task_id: &str,
    depends_on: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO task_deps (task_id, depends_on) VALUES (?1, ?2)",
        params![task_id, depends_on],
    )?;
    Ok(())
}

pub fn get_task_deps(conn: &Connection, task_id: &str) -> Result<Vec<String>, rusqlite::Error> {
    let mut stmt = conn.prepare("SELECT depends_on FROM task_deps WHERE task_id = ?1")?;
    let rows = stmt.query_map(params![task_id], |row| row.get(0))?;
    rows.collect()
}

pub fn next_unblocked(conn: &Connection, session_id: &str) -> Result<Vec<Task>, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {TASK_COLUMNS}
         FROM tasks t
         WHERE t.session_id = ?1
           AND t.status = 'pending'
           AND NOT EXISTS (
               SELECT 1 FROM task_deps td
               JOIN tasks dep ON td.depends_on = dep.id
               WHERE td.task_id = t.id AND dep.status != 'done'
           )
         ORDER BY t.priority DESC, t.created_at ASC"
    ))?;
    let rows = stmt.query_map(params![session_id], row_to_task)?;
    rows.collect()
}

// ---- Agents ----

const AGENT_COLUMNS: &str = "id, session_id, parent_agent_id, task_id, role, model, mode, status,
     token_input, token_output, created_at, finished_at";

pub fn create_agent(
    conn: &Connection,
    session_id: &str,
    parent_id: Option<&str>,
    role: &str,
    model: &str,
    mode: &str,
) -> Result<Agent, rusqlite::Error> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO agents (id, session_id, parent_agent_id, role, model, mode)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, session_id, parent_id, role, model, mode],
    )?;
    conn.query_row(
        &format!("SELECT {AGENT_COLUMNS} FROM agents WHERE id = ?1"),
        params![id],
        row_to_agent,
    )
}

pub fn get_agent(conn: &Connection, id: &str) -> Result<Option<Agent>, rusqlite::Error> {
    conn.query_row(
        &format!("SELECT {AGENT_COLUMNS} FROM agents WHERE id = ?1"),
        params![id],
        row_to_agent,
    )
    .optional()
}

pub fn update_agent_status(
    conn: &Connection,
    id: &str,
    status: &str,
) -> Result<(), rusqlite::Error> {
    let finished_at_clause = match status {
        "complete" | "failed" => ", finished_at = CURRENT_TIMESTAMP",
        _ => "",
    };
    conn.execute(
        &format!("UPDATE agents SET status = ?1{finished_at_clause} WHERE id = ?2"),
        params![status, id],
    )?;
    Ok(())
}

pub fn update_agent_tokens(
    conn: &Connection,
    id: &str,
    input_tokens: i64,
    output_tokens: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE agents
         SET token_input = token_input + ?1, token_output = token_output + ?2
         WHERE id = ?3",
        params![input_tokens, output_tokens, id],
    )?;
    Ok(())
}

// ---- Modes ----

const MODE_COLUMNS: &str = "name, description, system_prompt, default_model, allowed_tools,
     origin, version, created_at, updated_at";

pub fn insert_mode(conn: &Connection, mode: &Mode) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO modes (name, description, system_prompt, default_model,
                            allowed_tools, origin, version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            mode.name,
            mode.description,
            mode.system_prompt,
            mode.default_model,
            mode.allowed_tools,
            mode.origin,
            mode.version,
        ],
    )?;
    Ok(())
}

pub fn get_mode(conn: &Connection, name: &str) -> Result<Option<Mode>, rusqlite::Error> {
    conn.query_row(
        &format!("SELECT {MODE_COLUMNS} FROM modes WHERE name = ?1"),
        params![name],
        row_to_mode,
    )
    .optional()
}

pub fn list_modes(conn: &Connection) -> Result<Vec<Mode>, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {MODE_COLUMNS} FROM modes ORDER BY name ASC"
    ))?;
    let rows = stmt.query_map([], row_to_mode)?;
    rows.collect()
}

pub struct ModeUpdate<'a> {
    pub description: Option<&'a str>,
    pub system_prompt: Option<&'a str>,
    pub default_model: Option<Option<&'a str>>,
    pub allowed_tools: Option<&'a str>,
}

pub fn update_mode(
    conn: &Connection,
    name: &str,
    changes: &ModeUpdate,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE modes SET
            description = COALESCE(?2, description),
            system_prompt = COALESCE(?3, system_prompt),
            default_model = CASE WHEN ?4 THEN ?5 ELSE default_model END,
            allowed_tools = COALESCE(?6, allowed_tools),
            version = version + 1,
            updated_at = CURRENT_TIMESTAMP
         WHERE name = ?1",
        params![
            name,
            changes.description,
            changes.system_prompt,
            changes.default_model.is_some(),
            changes.default_model.flatten(),
            changes.allowed_tools,
        ],
    )?;
    Ok(())
}

pub fn delete_mode(conn: &Connection, name: &str) -> Result<(), rusqlite::Error> {
    conn.execute("DELETE FROM modes WHERE name = ?1", params![name])?;
    Ok(())
}

// ---- Recommendations ----

const RECOMMENDATION_COLUMNS: &str =
    "id, trigger_pattern, recommendation, action_type, action_payload,
     status, applied_at, reverted_at, created_at";

pub fn insert_recommendation(
    conn: &Connection,
    trigger_pattern: &str,
    recommendation: &str,
    action_type: &str,
    action_payload: &str,
) -> Result<Recommendation, rusqlite::Error> {
    conn.execute(
        "INSERT INTO recommendations (trigger_pattern, recommendation, action_type, action_payload)
         VALUES (?1, ?2, ?3, ?4)",
        params![trigger_pattern, recommendation, action_type, action_payload],
    )?;
    let id = conn.last_insert_rowid();
    conn.query_row(
        &format!("SELECT {RECOMMENDATION_COLUMNS} FROM recommendations WHERE id = ?1"),
        params![id],
        row_to_recommendation,
    )
}

pub fn list_recommendations(
    conn: &Connection,
    status_filter: Option<&str>,
) -> Result<Vec<Recommendation>, rusqlite::Error> {
    match status_filter {
        Some(status) => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {RECOMMENDATION_COLUMNS} FROM recommendations
                 WHERE status = ?1
                 ORDER BY created_at DESC"
            ))?;
            let rows = stmt.query_map(params![status], row_to_recommendation)?;
            rows.collect()
        }
        None => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {RECOMMENDATION_COLUMNS} FROM recommendations
                 ORDER BY created_at DESC"
            ))?;
            let rows = stmt.query_map([], row_to_recommendation)?;
            rows.collect()
        }
    }
}

pub fn update_recommendation_status(
    conn: &Connection,
    id: i64,
    status: &str,
) -> Result<(), rusqlite::Error> {
    let timestamp_clause = match status {
        "applied" => ", applied_at = CURRENT_TIMESTAMP",
        "reverted" => ", reverted_at = CURRENT_TIMESTAMP",
        _ => "",
    };
    conn.execute(
        &format!("UPDATE recommendations SET status = ?1{timestamp_clause} WHERE id = ?2"),
        params![status, id],
    )?;
    Ok(())
}

// ---- Stats Rollups ----

const ROLLUP_COLUMNS: &str = "id, period_start, period_end, scope, scope_id, metric, value";

pub fn insert_rollup(
    conn: &Connection,
    scope: &str,
    scope_id: Option<&str>,
    period_start: &str,
    period_end: &str,
    metric: &str,
    value: f64,
) -> Result<StatsRollup, rusqlite::Error> {
    conn.execute(
        "INSERT INTO stats_rollups (scope, scope_id, period_start, period_end, metric, value)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![scope, scope_id, period_start, period_end, metric, value],
    )?;
    let id = conn.last_insert_rowid();
    conn.query_row(
        &format!("SELECT {ROLLUP_COLUMNS} FROM stats_rollups WHERE id = ?1"),
        params![id],
        row_to_stats_rollup,
    )
}

pub fn query_rollups(
    conn: &Connection,
    scope: &str,
    scope_id: Option<&str>,
    metric: &str,
    since: &str,
) -> Result<Vec<StatsRollup>, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ROLLUP_COLUMNS} FROM stats_rollups
         WHERE scope = ?1 AND (?2 IS NULL OR scope_id = ?2)
           AND metric = ?3 AND period_start >= ?4
         ORDER BY period_start ASC"
    ))?;
    let rows = stmt.query_map(params![scope, scope_id, metric, since], row_to_stats_rollup)?;
    rows.collect()
}

// ---- UX Agent State ----

pub fn get_ux_state(
    conn: &Connection,
    scope: &str,
) -> Result<Option<UxAgentState>, rusqlite::Error> {
    conn.query_row(
        "SELECT scope, last_event_id, last_event_at, updated_at
         FROM ux_agent_state WHERE scope = ?1",
        params![scope],
        row_to_ux_agent_state,
    )
    .optional()
}

pub fn update_ux_state(
    conn: &Connection,
    scope: &str,
    last_event_id: &str,
    last_event_at: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO ux_agent_state (scope, last_event_id, last_event_at, updated_at)
         VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
         ON CONFLICT(scope) DO UPDATE SET
            last_event_id = ?2,
            last_event_at = ?3,
            updated_at = CURRENT_TIMESTAMP",
        params![scope, last_event_id, last_event_at],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::migrations;
    use super::*;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        migrations::run_migrations(&conn).unwrap();
        conn
    }

    fn create_test_session(conn: &Connection) -> Session {
        create_session(conn, "/tmp/test-project").unwrap()
    }

    // ---- Session tests ----

    #[test]
    fn test_create_and_get_session() {
        let conn = setup();
        let session = create_test_session(&conn);
        assert_eq!(session.project_path, "/tmp/test-project");
        assert!(!session.id.is_empty());

        let fetched = get_session(&conn, &session.id).unwrap().unwrap();
        assert_eq!(fetched.id, session.id);
        assert_eq!(fetched.project_path, session.project_path);
    }

    #[test]
    fn test_get_session_not_found() {
        let conn = setup();
        let result = get_session(&conn, "nonexistent").unwrap();
        assert!(result.is_none());
    }

    // ---- Event tests ----

    #[test]
    fn test_insert_and_query_events() {
        let conn = setup();
        let session = create_test_session(&conn);

        let e1 = insert_event(
            &conn,
            &session.id,
            None,
            "prompt_submitted",
            r#"{"prompt":"hello"}"#,
        )
        .unwrap();
        assert_eq!(e1.kind, "prompt_submitted");
        assert_eq!(e1.session_id, session.id);
        assert!(e1.agent_id.is_none());

        let e2 = insert_event(
            &conn,
            &session.id,
            Some("agent-1"),
            "tool_called",
            r#"{"tool":"fs_read"}"#,
        )
        .unwrap();
        assert_eq!(e2.agent_id.as_deref(), Some("agent-1"));

        let since = events_since(&conn, &session.id, "2000-01-01").unwrap();
        assert_eq!(since.len(), 2);
    }

    #[test]
    fn test_events_by_kind() {
        let conn = setup();
        let session = create_test_session(&conn);

        insert_event(&conn, &session.id, None, "prompt_submitted", "{}").unwrap();
        insert_event(&conn, &session.id, None, "tool_called", "{}").unwrap();
        insert_event(&conn, &session.id, None, "prompt_submitted", "{}").unwrap();

        let results = events_by_kind(&conn, "prompt_submitted", "2000-01-01").unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.kind == "prompt_submitted"));
    }

    // ---- Task tests ----

    #[test]
    fn test_create_and_get_task() {
        let conn = setup();
        let session = create_test_session(&conn);

        let task = create_task(
            &conn,
            &session.id,
            "Build auth",
            Some("Implement authentication"),
            None,
            &Priority::High,
            "main",
            "abc123",
            "main",
        )
        .unwrap();

        assert_eq!(task.title, "Build auth");
        assert_eq!(
            task.description.as_deref(),
            Some("Implement authentication")
        );
        assert_eq!(task.status, TaskStatus::Pending);
        assert_eq!(task.priority, Priority::High);
        assert_eq!(task.base_ref, "main");
        assert_eq!(task.base_commit, "abc123");

        let fetched = get_task(&conn, &task.id).unwrap().unwrap();
        assert_eq!(fetched.id, task.id);
    }

    #[test]
    fn test_list_tasks_with_filter() {
        let conn = setup();
        let session = create_test_session(&conn);

        let t1 = create_task(
            &conn,
            &session.id,
            "Task 1",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();
        create_task(
            &conn,
            &session.id,
            "Task 2",
            None,
            None,
            &Priority::Low,
            "main",
            "abc",
            "main",
        )
        .unwrap();

        // Mark t1 as in_progress
        update_task_status(&conn, &t1.id, &TaskStatus::InProgress, None, None).unwrap();

        let all = list_tasks(&conn, &session.id, None).unwrap();
        assert_eq!(all.len(), 2);

        let pending = list_tasks(&conn, &session.id, Some(&TaskStatus::Pending)).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].title, "Task 2");

        let in_progress = list_tasks(&conn, &session.id, Some(&TaskStatus::InProgress)).unwrap();
        assert_eq!(in_progress.len(), 1);
        assert_eq!(in_progress[0].title, "Task 1");
    }

    #[test]
    fn test_update_task_status_done_sets_completed_at() {
        let conn = setup();
        let session = create_test_session(&conn);

        let task = create_task(
            &conn,
            &session.id,
            "Task",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();
        assert!(task.completed_at.is_none());

        update_task_status(
            &conn,
            &task.id,
            &TaskStatus::Done,
            Some("success"),
            Some(r#"{"summary":"done"}"#),
        )
        .unwrap();

        let updated = get_task(&conn, &task.id).unwrap().unwrap();
        assert_eq!(updated.status, TaskStatus::Done);
        assert_eq!(updated.outcome_kind.as_deref(), Some("success"));
        assert!(updated.completed_at.is_some());
    }

    #[test]
    fn test_task_deps() {
        let conn = setup();
        let session = create_test_session(&conn);

        let t1 = create_task(
            &conn,
            &session.id,
            "Task A",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();
        let t2 = create_task(
            &conn,
            &session.id,
            "Task B",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();

        add_task_dep(&conn, &t2.id, &t1.id).unwrap();

        let deps = get_task_deps(&conn, &t2.id).unwrap();
        assert_eq!(deps, vec![t1.id]);
    }

    #[test]
    fn test_next_unblocked() {
        let conn = setup();
        let session = create_test_session(&conn);

        // A has no deps → unblocked
        let a = create_task(
            &conn,
            &session.id,
            "A",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();
        // B depends on A → blocked
        let b = create_task(
            &conn,
            &session.id,
            "B",
            None,
            None,
            &Priority::High,
            "main",
            "abc",
            "main",
        )
        .unwrap();
        add_task_dep(&conn, &b.id, &a.id).unwrap();
        // C has no deps → unblocked
        let _c = create_task(
            &conn,
            &session.id,
            "C",
            None,
            None,
            &Priority::Low,
            "main",
            "abc",
            "main",
        )
        .unwrap();

        let unblocked = next_unblocked(&conn, &session.id).unwrap();
        let titles: Vec<&str> = unblocked.iter().map(|t| t.title.as_str()).collect();
        // A (medium) before C (low); B is blocked
        assert_eq!(titles, vec!["A", "C"]);

        // Mark A as done → B becomes unblocked
        update_task_status(&conn, &a.id, &TaskStatus::Done, None, None).unwrap();

        let unblocked = next_unblocked(&conn, &session.id).unwrap();
        let titles: Vec<&str> = unblocked.iter().map(|t| t.title.as_str()).collect();
        // B (high) before C (low); A is done
        assert_eq!(titles, vec!["B", "C"]);
    }

    // ---- Agent tests ----

    #[test]
    fn test_create_and_get_agent() {
        let conn = setup();
        let session = create_test_session(&conn);

        let agent = create_agent(
            &conn,
            &session.id,
            None,
            "implementation",
            "claude-sonnet-4-20250514",
            "implement",
        )
        .unwrap();

        assert_eq!(agent.role, "implementation");
        assert_eq!(agent.status, "spawning");
        assert_eq!(agent.token_input, 0);

        let fetched = get_agent(&conn, &agent.id).unwrap().unwrap();
        assert_eq!(fetched.id, agent.id);
    }

    #[test]
    fn test_update_agent_status_sets_finished_at() {
        let conn = setup();
        let session = create_test_session(&conn);

        let agent = create_agent(
            &conn,
            &session.id,
            None,
            "research",
            "claude-haiku-4-5-20251001",
            "research",
        )
        .unwrap();
        assert!(agent.finished_at.is_none());

        update_agent_status(&conn, &agent.id, "running").unwrap();
        let a = get_agent(&conn, &agent.id).unwrap().unwrap();
        assert_eq!(a.status, "running");
        assert!(a.finished_at.is_none());

        update_agent_status(&conn, &agent.id, "complete").unwrap();
        let a = get_agent(&conn, &agent.id).unwrap().unwrap();
        assert_eq!(a.status, "complete");
        assert!(a.finished_at.is_some());
    }

    #[test]
    fn test_update_agent_tokens() {
        let conn = setup();
        let session = create_test_session(&conn);

        let agent = create_agent(
            &conn,
            &session.id,
            None,
            "implementation",
            "claude-sonnet-4-20250514",
            "implement",
        )
        .unwrap();

        update_agent_tokens(&conn, &agent.id, 1000, 500).unwrap();
        let a = get_agent(&conn, &agent.id).unwrap().unwrap();
        assert_eq!(a.token_input, 1000);
        assert_eq!(a.token_output, 500);

        // Tokens are additive
        update_agent_tokens(&conn, &agent.id, 200, 100).unwrap();
        let a = get_agent(&conn, &agent.id).unwrap().unwrap();
        assert_eq!(a.token_input, 1200);
        assert_eq!(a.token_output, 600);
    }

    // ---- Mode tests ----

    #[test]
    fn test_mode_crud() {
        let conn = setup();

        let mode = Mode {
            name: "plan".into(),
            description: "Planning mode".into(),
            system_prompt: "You are a planner.".into(),
            default_model: None,
            allowed_tools: r#"["fs_read","git"]"#.into(),
            origin: "builtin".into(),
            version: 1,
            created_at: String::new(),
            updated_at: String::new(),
        };
        insert_mode(&conn, &mode).unwrap();

        let fetched = get_mode(&conn, "plan").unwrap().unwrap();
        assert_eq!(fetched.description, "Planning mode");
        assert!(fetched.default_model.is_none());

        let all = list_modes(&conn).unwrap();
        assert_eq!(all.len(), 1);

        delete_mode(&conn, "plan").unwrap();
        assert!(get_mode(&conn, "plan").unwrap().is_none());
    }

    #[test]
    fn test_update_mode_partial() {
        let conn = setup();

        let mode = Mode {
            name: "debug".into(),
            description: "Debug mode".into(),
            system_prompt: "You debug.".into(),
            default_model: None,
            allowed_tools: r#"["shell"]"#.into(),
            origin: "builtin".into(),
            version: 1,
            created_at: String::new(),
            updated_at: String::new(),
        };
        insert_mode(&conn, &mode).unwrap();

        // Update only description and set a default_model
        update_mode(
            &conn,
            "debug",
            &ModeUpdate {
                description: Some("Enhanced debug mode"),
                system_prompt: None,
                default_model: Some(Some("claude-opus-4-20250514")),
                allowed_tools: None,
            },
        )
        .unwrap();

        let updated = get_mode(&conn, "debug").unwrap().unwrap();
        assert_eq!(updated.description, "Enhanced debug mode");
        assert_eq!(updated.system_prompt, "You debug."); // unchanged
        assert_eq!(
            updated.default_model.as_deref(),
            Some("claude-opus-4-20250514")
        );
        assert_eq!(updated.allowed_tools, r#"["shell"]"#); // unchanged
        assert_eq!(updated.version, 2);

        // Clear default_model
        update_mode(
            &conn,
            "debug",
            &ModeUpdate {
                description: None,
                system_prompt: None,
                default_model: Some(None),
                allowed_tools: None,
            },
        )
        .unwrap();

        let updated = get_mode(&conn, "debug").unwrap().unwrap();
        assert!(updated.default_model.is_none());
        assert_eq!(updated.version, 3);
    }

    // ---- Recommendation tests ----

    #[test]
    fn test_recommendation_crud() {
        let conn = setup();

        let rec = insert_recommendation(
            &conn,
            "plan_rejected x3",
            "Switch planning model to opus",
            "model_change",
            r#"{"model":"claude-opus-4-20250514"}"#,
        )
        .unwrap();
        assert_eq!(rec.status, "pending");
        assert!(rec.applied_at.is_none());

        let all = list_recommendations(&conn, None).unwrap();
        assert_eq!(all.len(), 1);

        let pending = list_recommendations(&conn, Some("pending")).unwrap();
        assert_eq!(pending.len(), 1);

        update_recommendation_status(&conn, rec.id, "applied").unwrap();

        let applied = list_recommendations(&conn, Some("applied")).unwrap();
        assert_eq!(applied.len(), 1);
        assert!(applied[0].applied_at.is_some());

        let pending = list_recommendations(&conn, Some("pending")).unwrap();
        assert!(pending.is_empty());
    }

    // ---- Stats rollup tests ----

    #[test]
    fn test_stats_rollup_crud() {
        let conn = setup();

        let r1 = insert_rollup(
            &conn,
            "session",
            Some("s-1"),
            "2026-01-01T00:00:00",
            "2026-01-02T00:00:00",
            "cost.usd",
            3.50,
        )
        .unwrap();
        assert_eq!(r1.scope, "session");
        assert_eq!(r1.value, 3.50);

        insert_rollup(
            &conn,
            "session",
            Some("s-1"),
            "2026-01-02T00:00:00",
            "2026-01-03T00:00:00",
            "cost.usd",
            2.00,
        )
        .unwrap();

        let results = query_rollups(
            &conn,
            "session",
            Some("s-1"),
            "cost.usd",
            "2026-01-01T00:00:00",
        )
        .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].value, 3.50);
        assert_eq!(results[1].value, 2.00);

        // Filter by since
        let results = query_rollups(
            &conn,
            "session",
            Some("s-1"),
            "cost.usd",
            "2026-01-02T00:00:00",
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, 2.00);
    }

    // ---- UX Agent State tests ----

    #[test]
    fn test_ux_agent_state() {
        let conn = setup();

        assert!(get_ux_state(&conn, "global").unwrap().is_none());

        update_ux_state(&conn, "global", "evt-1", "2026-01-01T00:00:00").unwrap();

        let state = get_ux_state(&conn, "global").unwrap().unwrap();
        assert_eq!(state.last_event_id.as_deref(), Some("evt-1"));
        assert_eq!(state.last_event_at.as_deref(), Some("2026-01-01T00:00:00"));

        // Upsert updates existing row
        update_ux_state(&conn, "global", "evt-5", "2026-01-02T00:00:00").unwrap();

        let state = get_ux_state(&conn, "global").unwrap().unwrap();
        assert_eq!(state.last_event_id.as_deref(), Some("evt-5"));
        assert_eq!(state.last_event_at.as_deref(), Some("2026-01-02T00:00:00"));
    }
}
