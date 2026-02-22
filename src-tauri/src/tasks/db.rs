use std::collections::HashMap;

use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use uuid::Uuid;

use super::types::*;

const SQLITE_TIMESTAMP_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

const TASK_COLUMNS: &str = "id, session_id, title, description, status, priority, assigned_agent, \
     parent_task, worktree_branch, base_ref, base_commit, merge_target_ref, outcome_kind, \
     outcome_data, engagement_override, cost_usd, created_at, updated_at";

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending',
    priority TEXT NOT NULL DEFAULT 'medium',
    assigned_agent TEXT,
    parent_task TEXT,
    worktree_branch TEXT,
    base_ref TEXT NOT NULL DEFAULT 'main',
    base_commit TEXT NOT NULL DEFAULT '',
    merge_target_ref TEXT NOT NULL DEFAULT 'main',
    outcome_kind TEXT,
    outcome_data TEXT,
    engagement_override TEXT,
    cost_usd REAL NOT NULL DEFAULT 0.0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS task_deps (
    task_id TEXT NOT NULL,
    depends_on_task_id TEXT NOT NULL,
    PRIMARY KEY (task_id, depends_on_task_id),
    FOREIGN KEY (task_id) REFERENCES tasks(id),
    FOREIGN KEY (depends_on_task_id) REFERENCES tasks(id)
);

CREATE INDEX IF NOT EXISTS idx_tasks_session_id ON tasks(session_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
CREATE INDEX IF NOT EXISTS idx_tasks_parent_task ON tasks(parent_task);
CREATE INDEX IF NOT EXISTS idx_task_deps_task_id ON task_deps(task_id);
CREATE INDEX IF NOT EXISTS idx_task_deps_depends_on ON task_deps(depends_on_task_id);
"#;

pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA_SQL)
}

pub fn insert_task(conn: &Connection, task: &Task) -> rusqlite::Result<()> {
    let tx: Transaction<'_> = conn.unchecked_transaction()?;
    insert_task_in_transaction(&tx, task)?;
    tx.commit()
}

pub fn insert_task_in_transaction(tx: &Transaction<'_>, task: &Task) -> rusqlite::Result<()> {
    insert_task_rows(tx, task)
}

fn insert_task_rows(conn: &Connection, task: &Task) -> rusqlite::Result<()> {
    let (outcome_kind, outcome_data) = outcome_to_columns(task.outcome.as_ref())?;

    conn.execute(
        "INSERT INTO tasks (
            id, session_id, title, description, status, priority, assigned_agent, parent_task,
            worktree_branch, base_ref, base_commit, merge_target_ref, outcome_kind, outcome_data,
            engagement_override, cost_usd, created_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
            ?9, ?10, ?11, ?12, ?13, ?14,
            ?15, ?16, ?17, ?18
        )",
        params![
            task.id.to_string(),
            task.session_id.to_string(),
            task.title,
            task.description,
            task.status.as_str(),
            task.priority.as_str(),
            task.assigned_agent.map(|id| id.to_string()),
            task.parent_task.map(|id| id.to_string()),
            task.worktree_branch,
            task.base_ref,
            task.base_commit,
            task.merge_target_ref,
            outcome_kind,
            outcome_data,
            task.engagement_override.map(|level| level.as_str()),
            task.cost_usd,
            format_sqlite_timestamp(task.created_at),
            format_sqlite_timestamp(task.updated_at),
        ],
    )?;

    for dep in &task.depends_on {
        conn.execute(
            "INSERT INTO task_deps (task_id, depends_on_task_id) VALUES (?1, ?2)",
            params![task.id.to_string(), dep.to_string()],
        )?;
    }

    Ok(())
}

pub fn get_task(conn: &Connection, task_id: Uuid) -> rusqlite::Result<Option<Task>> {
    let task = conn
        .query_row(
            &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
            params![task_id.to_string()],
            row_to_task,
        )
        .optional()?;

    match task {
        Some(mut task) => {
            task.depends_on = load_dependencies(conn, task.id)?;
            Ok(Some(task))
        }
        None => Ok(None),
    }
}

pub fn list_tasks(
    conn: &Connection,
    session_id: Uuid,
    status_filter: Option<TaskStatus>,
) -> rusqlite::Result<Vec<Task>> {
    let mut tasks = match status_filter {
        Some(status) => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TASK_COLUMNS}
                 FROM tasks
                 WHERE session_id = ?1 AND status = ?2
                 ORDER BY created_at ASC"
            ))?;
            let rows = stmt.query_map(
                params![session_id.to_string(), status.as_str()],
                row_to_task,
            )?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        }
        None => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TASK_COLUMNS}
                 FROM tasks
                 WHERE session_id = ?1
                 ORDER BY created_at ASC"
            ))?;
            let rows = stmt.query_map(params![session_id.to_string()], row_to_task)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        }
    };

    let mut dep_map = load_dependency_map(conn, session_id)?;
    for task in &mut tasks {
        task.depends_on = dep_map.remove(&task.id).unwrap_or_default();
    }

    Ok(tasks)
}

pub fn update_task_status(
    conn: &Connection,
    task_id: Uuid,
    status: TaskStatus,
    outcome: Option<&TaskOutcome>,
) -> rusqlite::Result<()> {
    let (outcome_kind, outcome_data) = outcome_to_columns(outcome)?;
    conn.execute(
        "UPDATE tasks
         SET status = ?1, outcome_kind = ?2, outcome_data = ?3, updated_at = CURRENT_TIMESTAMP
         WHERE id = ?4",
        params![
            status.as_str(),
            outcome_kind,
            outcome_data,
            task_id.to_string(),
        ],
    )?;
    Ok(())
}

pub fn update_task_engagement(
    conn: &Connection,
    task_id: Uuid,
    engagement: Option<EngagementLevel>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tasks
         SET engagement_override = ?1, updated_at = CURRENT_TIMESTAMP
         WHERE id = ?2",
        params![engagement.map(|level| level.as_str()), task_id.to_string()],
    )?;
    Ok(())
}

pub fn get_task_tree(
    conn: &Connection,
    session_id: Uuid,
) -> rusqlite::Result<(Vec<Task>, Vec<(Uuid, Uuid)>)> {
    let tasks = list_tasks(conn, session_id, None)?;

    let mut stmt = conn.prepare(
        "SELECT td.task_id, td.depends_on_task_id
         FROM task_deps td
         JOIN tasks t ON t.id = td.task_id
         WHERE t.session_id = ?1",
    )?;
    let rows = stmt.query_map(params![session_id.to_string()], |row| {
        let task_id_raw: String = row.get(0)?;
        let depends_on_raw: String = row.get(1)?;
        Ok((
            parse_uuid(&task_id_raw, "task_deps.task_id")?,
            parse_uuid(&depends_on_raw, "task_deps.depends_on_task_id")?,
        ))
    })?;
    let edges = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    Ok((tasks, edges))
}

pub fn next_unblocked(conn: &Connection, session_id: Uuid) -> rusqlite::Result<Vec<Task>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {TASK_COLUMNS}
         FROM tasks t
         WHERE t.session_id = ?1
           AND t.status = 'pending'
           AND NOT EXISTS (
               SELECT 1
               FROM task_deps td
               JOIN tasks dep ON dep.id = td.depends_on_task_id
               WHERE td.task_id = t.id
                 AND dep.status != 'done'
           )
         ORDER BY
           CASE t.priority
             WHEN 'critical' THEN 4
             WHEN 'high' THEN 3
             WHEN 'medium' THEN 2
             WHEN 'low' THEN 1
             ELSE 0
           END DESC,
           t.created_at ASC"
    ))?;
    let rows = stmt.query_map(params![session_id.to_string()], row_to_task)?;
    let mut tasks = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    let mut dep_map = load_dependency_map(conn, session_id)?;
    for task in &mut tasks {
        task.depends_on = dep_map.remove(&task.id).unwrap_or_default();
    }

    Ok(tasks)
}

pub fn find_dependents(conn: &Connection, task_id: Uuid) -> rusqlite::Result<Vec<Uuid>> {
    let mut stmt = conn.prepare(
        "SELECT task_id
         FROM task_deps
         WHERE depends_on_task_id = ?1
         ORDER BY task_id ASC",
    )?;
    let rows = stmt.query_map(params![task_id.to_string()], |row| {
        let task_id_raw: String = row.get(0)?;
        parse_uuid(&task_id_raw, "task_deps.task_id")
    })?;
    rows.collect()
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let id_raw: String = row.get(0)?;
    let session_id_raw: String = row.get(1)?;
    let status_raw: String = row.get(4)?;
    let priority_raw: String = row.get(5)?;
    let assigned_agent_raw: Option<String> = row.get(6)?;
    let parent_task_raw: Option<String> = row.get(7)?;
    let outcome_kind: Option<String> = row.get(12)?;
    let outcome_data: Option<String> = row.get(13)?;
    let engagement_override_raw: Option<String> = row.get(14)?;
    let created_at_raw: String = row.get(16)?;
    let updated_at_raw: String = row.get(17)?;

    Ok(Task {
        id: parse_uuid(&id_raw, "tasks.id")?,
        session_id: parse_uuid(&session_id_raw, "tasks.session_id")?,
        title: row.get(2)?,
        description: row.get(3)?,
        status: TaskStatus::try_from(status_raw.as_str()).map_err(invalid_data)?,
        priority: Priority::try_from(priority_raw.as_str()).map_err(invalid_data)?,
        assigned_agent: parse_optional_uuid(assigned_agent_raw.as_deref(), "tasks.assigned_agent")?,
        parent_task: parse_optional_uuid(parent_task_raw.as_deref(), "tasks.parent_task")?,
        depends_on: Vec::new(),
        worktree_branch: row.get(8)?,
        base_ref: row.get(9)?,
        base_commit: row.get(10)?,
        merge_target_ref: row.get(11)?,
        outcome: outcome_from_columns(outcome_kind, outcome_data)?,
        engagement_override: match engagement_override_raw.as_deref() {
            Some(level) => Some(EngagementLevel::try_from(level).map_err(invalid_data)?),
            None => None,
        },
        cost_usd: row.get(15)?,
        created_at: parse_timestamp(&created_at_raw, "tasks.created_at")?,
        updated_at: parse_timestamp(&updated_at_raw, "tasks.updated_at")?,
    })
}

fn load_dependencies(conn: &Connection, task_id: Uuid) -> rusqlite::Result<Vec<Uuid>> {
    let mut stmt = conn.prepare(
        "SELECT depends_on_task_id
         FROM task_deps
         WHERE task_id = ?1
         ORDER BY depends_on_task_id ASC",
    )?;
    let rows = stmt.query_map(params![task_id.to_string()], |row| {
        let dep_raw: String = row.get(0)?;
        parse_uuid(&dep_raw, "task_deps.depends_on_task_id")
    })?;
    rows.collect()
}

fn load_dependency_map(
    conn: &Connection,
    session_id: Uuid,
) -> rusqlite::Result<HashMap<Uuid, Vec<Uuid>>> {
    let mut stmt = conn.prepare(
        "SELECT td.task_id, td.depends_on_task_id
         FROM task_deps td
         JOIN tasks t ON t.id = td.task_id
         WHERE t.session_id = ?1
         ORDER BY td.task_id ASC, td.depends_on_task_id ASC",
    )?;
    let rows = stmt.query_map(params![session_id.to_string()], |row| {
        let task_id_raw: String = row.get(0)?;
        let depends_on_raw: String = row.get(1)?;
        Ok((
            parse_uuid(&task_id_raw, "task_deps.task_id")?,
            parse_uuid(&depends_on_raw, "task_deps.depends_on_task_id")?,
        ))
    })?;

    let mut map: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for edge in rows {
        let (task_id, depends_on_task_id) = edge?;
        map.entry(task_id).or_default().push(depends_on_task_id);
    }
    Ok(map)
}

fn outcome_to_columns(
    outcome: Option<&TaskOutcome>,
) -> rusqlite::Result<(Option<String>, Option<String>)> {
    let Some(outcome) = outcome else {
        return Ok((None, None));
    };

    let encoded = serde_json::to_value(outcome)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let kind = encoded
        .get("kind")
        .and_then(|value| value.as_str())
        .ok_or_else(|| invalid_data("task outcome serialized without kind".to_string()))?;
    let data = encoded
        .get("data")
        .cloned()
        .ok_or_else(|| invalid_data("task outcome serialized without data".to_string()))?;
    let data_json = serde_json::to_string(&data)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

    Ok((Some(kind.to_string()), Some(data_json)))
}

fn outcome_from_columns(
    outcome_kind: Option<String>,
    outcome_data: Option<String>,
) -> rusqlite::Result<Option<TaskOutcome>> {
    match (outcome_kind, outcome_data) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => Err(invalid_data(
            "outcome_kind and outcome_data must either both be NULL or both be present".to_string(),
        )),
        (Some(kind), Some(data_json)) => {
            let data: serde_json::Value = serde_json::from_str(&data_json)
                .map_err(|e| invalid_data(format!("invalid task outcome_data JSON: {e}")))?;
            let combined = serde_json::json!({
                "kind": kind,
                "data": data,
            });
            let outcome: TaskOutcome = serde_json::from_value(combined)
                .map_err(|e| invalid_data(format!("invalid task outcome payload: {e}")))?;
            Ok(Some(outcome))
        }
    }
}

fn parse_uuid(value: &str, field: &str) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(value).map_err(|e| invalid_data(format!("invalid UUID in {field}: {e}")))
}

fn parse_optional_uuid(value: Option<&str>, field: &str) -> rusqlite::Result<Option<Uuid>> {
    match value {
        Some(raw) => parse_uuid(raw, field).map(Some),
        None => Ok(None),
    }
}

fn parse_timestamp(value: &str, field: &str) -> rusqlite::Result<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(value, SQLITE_TIMESTAMP_FORMAT)
        .map(|naive| naive.and_utc())
        .map_err(|e| invalid_data(format!("invalid timestamp in {field}: {e}")))
}

fn format_sqlite_timestamp(value: DateTime<Utc>) -> String {
    value.format(SQLITE_TIMESTAMP_FORMAT).to_string()
}

fn invalid_data(message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        )),
    )
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use rusqlite::Connection;
    use uuid::Uuid;

    use super::*;

    fn sample_task(session_id: Uuid) -> Task {
        let now = Utc::now();
        Task {
            id: Uuid::new_v4(),
            session_id,
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
            engagement_override: None,
            cost_usd: 0.0,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn update_task_engagement_handles_some_and_none() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        let task = sample_task(Uuid::new_v4());
        insert_task(&conn, &task).unwrap();

        update_task_engagement(&conn, task.id, Some(EngagementLevel::ReviewGates)).unwrap();
        let updated = get_task(&conn, task.id).unwrap().unwrap();
        assert_eq!(
            updated.engagement_override,
            Some(EngagementLevel::ReviewGates)
        );

        update_task_engagement(&conn, task.id, None).unwrap();
        let cleared = get_task(&conn, task.id).unwrap().unwrap();
        assert_eq!(cleared.engagement_override, None);
    }
}
