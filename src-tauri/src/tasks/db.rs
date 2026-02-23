use std::collections::HashMap;

use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use super::types::*;

const SQLITE_TIMESTAMP_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

const TASK_COLUMNS: &str = "id, session_id, title, description, status, priority, assigned_agent, \
     parent_task, worktree_branch, base_ref, base_commit, merge_target_ref, outcome_kind, \
     outcome_data, engagement_override, cost_usd, created_at, updated_at";

pub async fn insert_task(pool: &SqlitePool, task: &Task) -> Result<(), sqlx::Error> {
    insert_task_rows(pool, task).await
}

pub async fn insert_task_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task: &Task,
) -> Result<(), sqlx::Error> {
    insert_task_rows_tx(&mut **tx, task).await
}

async fn insert_task_rows(pool: &SqlitePool, task: &Task) -> Result<(), sqlx::Error> {
    let (outcome_kind, outcome_data) = outcome_to_columns(task.outcome.as_ref());

    sqlx::query(
        "INSERT INTO tasks (
            id, session_id, title, description, status, priority, assigned_agent, parent_task,
            worktree_branch, base_ref, base_commit, merge_target_ref, outcome_kind, outcome_data,
            engagement_override, cost_usd, created_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
            ?9, ?10, ?11, ?12, ?13, ?14,
            ?15, ?16, ?17, ?18
        )",
    )
    .bind(task.id.to_string())
    .bind(task.session_id.to_string())
    .bind(&task.title)
    .bind(&task.description)
    .bind(task.status.as_str())
    .bind(task.priority.as_str())
    .bind(task.assigned_agent.map(|id| id.to_string()))
    .bind(task.parent_task.map(|id| id.to_string()))
    .bind(&task.worktree_branch)
    .bind(&task.base_ref)
    .bind(&task.base_commit)
    .bind(&task.merge_target_ref)
    .bind(&outcome_kind)
    .bind(&outcome_data)
    .bind(task.engagement_override.map(|level| level.as_str()))
    .bind(task.cost_usd)
    .bind(format_sqlite_timestamp(task.created_at))
    .bind(format_sqlite_timestamp(task.updated_at))
    .execute(pool)
    .await?;

    for dep in &task.depends_on {
        sqlx::query("INSERT INTO task_deps (task_id, depends_on_task_id) VALUES (?1, ?2)")
            .bind(task.id.to_string())
            .bind(dep.to_string())
            .execute(pool)
            .await?;
    }

    Ok(())
}

async fn insert_task_rows_tx(
    conn: &mut sqlx::SqliteConnection,
    task: &Task,
) -> Result<(), sqlx::Error> {
    let (outcome_kind, outcome_data) = outcome_to_columns(task.outcome.as_ref());

    sqlx::query(
        "INSERT INTO tasks (
            id, session_id, title, description, status, priority, assigned_agent, parent_task,
            worktree_branch, base_ref, base_commit, merge_target_ref, outcome_kind, outcome_data,
            engagement_override, cost_usd, created_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
            ?9, ?10, ?11, ?12, ?13, ?14,
            ?15, ?16, ?17, ?18
        )",
    )
    .bind(task.id.to_string())
    .bind(task.session_id.to_string())
    .bind(&task.title)
    .bind(&task.description)
    .bind(task.status.as_str())
    .bind(task.priority.as_str())
    .bind(task.assigned_agent.map(|id| id.to_string()))
    .bind(task.parent_task.map(|id| id.to_string()))
    .bind(&task.worktree_branch)
    .bind(&task.base_ref)
    .bind(&task.base_commit)
    .bind(&task.merge_target_ref)
    .bind(&outcome_kind)
    .bind(&outcome_data)
    .bind(task.engagement_override.map(|level| level.as_str()))
    .bind(task.cost_usd)
    .bind(format_sqlite_timestamp(task.created_at))
    .bind(format_sqlite_timestamp(task.updated_at))
    .execute(&mut *conn)
    .await?;

    for dep in &task.depends_on {
        sqlx::query("INSERT INTO task_deps (task_id, depends_on_task_id) VALUES (?1, ?2)")
            .bind(task.id.to_string())
            .bind(dep.to_string())
            .execute(&mut *conn)
            .await?;
    }

    Ok(())
}

pub async fn get_task(pool: &SqlitePool, task_id: Uuid) -> Result<Option<Task>, sqlx::Error> {
    let row = sqlx::query_as::<_, TaskRow>(&format!(
        "SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"
    ))
    .bind(task_id.to_string())
    .fetch_optional(pool)
    .await?;

    match row {
        Some(row) => {
            let mut task = row_to_task(row)?;
            task.depends_on = load_dependencies(pool, task.id).await?;
            Ok(Some(task))
        }
        None => Ok(None),
    }
}

pub async fn list_tasks(
    pool: &SqlitePool,
    session_id: Uuid,
    status_filter: Option<TaskStatus>,
) -> Result<Vec<Task>, sqlx::Error> {
    let rows = match status_filter {
        Some(status) => {
            sqlx::query_as::<_, TaskRow>(&format!(
                "SELECT {TASK_COLUMNS}
                 FROM tasks
                 WHERE session_id = ?1 AND status = ?2
                 ORDER BY created_at ASC"
            ))
            .bind(session_id.to_string())
            .bind(status.as_str())
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, TaskRow>(&format!(
                "SELECT {TASK_COLUMNS}
                 FROM tasks
                 WHERE session_id = ?1
                 ORDER BY created_at ASC"
            ))
            .bind(session_id.to_string())
            .fetch_all(pool)
            .await?
        }
    };

    let mut tasks: Vec<Task> = rows
        .into_iter()
        .map(row_to_task)
        .collect::<Result<Vec<_>, _>>()?;

    let mut dep_map = load_dependency_map(pool, session_id).await?;
    for task in &mut tasks {
        task.depends_on = dep_map.remove(&task.id).unwrap_or_default();
    }

    Ok(tasks)
}

pub async fn update_task_status(
    pool: &SqlitePool,
    task_id: Uuid,
    status: TaskStatus,
    outcome: Option<&TaskOutcome>,
) -> Result<(), sqlx::Error> {
    let (outcome_kind, outcome_data) = outcome_to_columns(outcome);
    sqlx::query(
        "UPDATE tasks
         SET status = ?1, outcome_kind = ?2, outcome_data = ?3, updated_at = CURRENT_TIMESTAMP
         WHERE id = ?4",
    )
    .bind(status.as_str())
    .bind(&outcome_kind)
    .bind(&outcome_data)
    .bind(task_id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_task_engagement(
    pool: &SqlitePool,
    task_id: Uuid,
    engagement: Option<EngagementLevel>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE tasks
         SET engagement_override = ?1, updated_at = CURRENT_TIMESTAMP
         WHERE id = ?2",
    )
    .bind(engagement.map(|level| level.as_str()))
    .bind(task_id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_task_tree(
    pool: &SqlitePool,
    session_id: Uuid,
) -> Result<(Vec<Task>, Vec<(Uuid, Uuid)>), sqlx::Error> {
    let tasks = list_tasks(pool, session_id, None).await?;

    let edge_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT td.task_id, td.depends_on_task_id
         FROM task_deps td
         JOIN tasks t ON t.id = td.task_id
         WHERE t.session_id = ?1",
    )
    .bind(session_id.to_string())
    .fetch_all(pool)
    .await?;

    let edges: Vec<(Uuid, Uuid)> = edge_rows
        .into_iter()
        .map(|(tid, did)| {
            Ok((
                parse_uuid(&tid, "task_deps.task_id")?,
                parse_uuid(&did, "task_deps.depends_on_task_id")?,
            ))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;

    Ok((tasks, edges))
}

pub async fn next_unblocked(
    pool: &SqlitePool,
    session_id: Uuid,
) -> Result<Vec<Task>, sqlx::Error> {
    let rows = sqlx::query_as::<_, TaskRow>(&format!(
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
    ))
    .bind(session_id.to_string())
    .fetch_all(pool)
    .await?;

    let mut tasks: Vec<Task> = rows
        .into_iter()
        .map(row_to_task)
        .collect::<Result<Vec<_>, _>>()?;

    let mut dep_map = load_dependency_map(pool, session_id).await?;
    for task in &mut tasks {
        task.depends_on = dep_map.remove(&task.id).unwrap_or_default();
    }

    Ok(tasks)
}

pub async fn find_dependents(pool: &SqlitePool, task_id: Uuid) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT task_id
         FROM task_deps
         WHERE depends_on_task_id = ?1
         ORDER BY task_id ASC",
    )
    .bind(task_id.to_string())
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|(tid,)| parse_uuid(&tid, "task_deps.task_id"))
        .collect()
}

// ---- Internal types and helpers ----

#[derive(sqlx::FromRow)]
struct TaskRow {
    id: String,
    session_id: String,
    title: String,
    description: String,
    status: String,
    priority: String,
    assigned_agent: Option<String>,
    parent_task: Option<String>,
    worktree_branch: Option<String>,
    base_ref: String,
    base_commit: String,
    merge_target_ref: String,
    outcome_kind: Option<String>,
    outcome_data: Option<String>,
    engagement_override: Option<String>,
    cost_usd: f64,
    created_at: String,
    updated_at: String,
}

fn row_to_task(row: TaskRow) -> Result<Task, sqlx::Error> {
    Ok(Task {
        id: parse_uuid(&row.id, "tasks.id")?,
        session_id: parse_uuid(&row.session_id, "tasks.session_id")?,
        title: row.title,
        description: row.description,
        status: TaskStatus::try_from(row.status.as_str()).map_err(invalid_data)?,
        priority: Priority::try_from(row.priority.as_str()).map_err(invalid_data)?,
        assigned_agent: parse_optional_uuid(row.assigned_agent.as_deref(), "tasks.assigned_agent")?,
        parent_task: parse_optional_uuid(row.parent_task.as_deref(), "tasks.parent_task")?,
        depends_on: Vec::new(),
        worktree_branch: row.worktree_branch,
        base_ref: row.base_ref,
        base_commit: row.base_commit,
        merge_target_ref: row.merge_target_ref,
        outcome: outcome_from_columns(row.outcome_kind, row.outcome_data)?,
        engagement_override: match row.engagement_override.as_deref() {
            Some(level) => Some(EngagementLevel::try_from(level).map_err(invalid_data)?),
            None => None,
        },
        cost_usd: row.cost_usd,
        created_at: parse_timestamp(&row.created_at, "tasks.created_at")?,
        updated_at: parse_timestamp(&row.updated_at, "tasks.updated_at")?,
    })
}

async fn load_dependencies(pool: &SqlitePool, task_id: Uuid) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT depends_on_task_id
         FROM task_deps
         WHERE task_id = ?1
         ORDER BY depends_on_task_id ASC",
    )
    .bind(task_id.to_string())
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|(dep,)| parse_uuid(&dep, "task_deps.depends_on_task_id"))
        .collect()
}

async fn load_dependency_map(
    pool: &SqlitePool,
    session_id: Uuid,
) -> Result<HashMap<Uuid, Vec<Uuid>>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT td.task_id, td.depends_on_task_id
         FROM task_deps td
         JOIN tasks t ON t.id = td.task_id
         WHERE t.session_id = ?1
         ORDER BY td.task_id ASC, td.depends_on_task_id ASC",
    )
    .bind(session_id.to_string())
    .fetch_all(pool)
    .await?;

    let mut map: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (task_id_raw, depends_on_raw) in rows {
        let task_id = parse_uuid(&task_id_raw, "task_deps.task_id")?;
        let depends_on_task_id = parse_uuid(&depends_on_raw, "task_deps.depends_on_task_id")?;
        map.entry(task_id).or_default().push(depends_on_task_id);
    }
    Ok(map)
}

fn outcome_to_columns(outcome: Option<&TaskOutcome>) -> (Option<String>, Option<String>) {
    let Some(outcome) = outcome else {
        return (None, None);
    };

    let encoded = serde_json::to_value(outcome).expect("TaskOutcome serialization should not fail");
    let kind = encoded
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let data = encoded
        .get("data")
        .map(|v| serde_json::to_string(v).unwrap_or_default())
        .unwrap_or_default();

    (Some(kind), Some(data))
}

fn outcome_from_columns(
    outcome_kind: Option<String>,
    outcome_data: Option<String>,
) -> Result<Option<TaskOutcome>, sqlx::Error> {
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

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, sqlx::Error> {
    Uuid::parse_str(value).map_err(|e| invalid_data(format!("invalid UUID in {field}: {e}")))
}

fn parse_optional_uuid(value: Option<&str>, field: &str) -> Result<Option<Uuid>, sqlx::Error> {
    match value {
        Some(raw) => parse_uuid(raw, field).map(Some),
        None => Ok(None),
    }
}

fn parse_timestamp(value: &str, field: &str) -> Result<DateTime<Utc>, sqlx::Error> {
    NaiveDateTime::parse_from_str(value, SQLITE_TIMESTAMP_FORMAT)
        .map(|naive| naive.and_utc())
        .map_err(|e| invalid_data(format!("invalid timestamp in {field}: {e}")))
}

fn format_sqlite_timestamp(value: DateTime<Utc>) -> String {
    value.format(SQLITE_TIMESTAMP_FORMAT).to_string()
}

fn invalid_data(message: String) -> sqlx::Error {
    sqlx::Error::Protocol(message)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::db::test_pool;

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

    #[tokio::test]
    async fn update_task_engagement_handles_some_and_none() {
        let pool = test_pool().await;

        let task = sample_task(Uuid::new_v4());
        insert_task(&pool, &task).await.unwrap();

        update_task_engagement(&pool, task.id, Some(EngagementLevel::ReviewGates))
            .await
            .unwrap();
        let updated = get_task(&pool, task.id).await.unwrap().unwrap();
        assert_eq!(
            updated.engagement_override,
            Some(EngagementLevel::ReviewGates)
        );

        update_task_engagement(&pool, task.id, None).await.unwrap();
        let cleared = get_task(&pool, task.id).await.unwrap().unwrap();
        assert_eq!(cleared.engagement_override, None);
    }
}
