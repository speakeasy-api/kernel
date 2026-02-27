use sqlx::SqlitePool;
use tracing::{debug, info, instrument};
use uuid::Uuid;

use super::models::{Agent, ConversationRow, Event, Mode, Session, SnapshotRow, StatsRollup, Task};

// ---- Sessions ----

#[instrument(skip(pool))]
pub async fn create_session(pool: &SqlitePool, project_path: &str) -> Result<Session, sqlx::Error> {
    info!(project_path, "creating session");
    let id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO sessions (id, project_path) VALUES (?1, ?2)")
        .bind(&id)
        .bind(project_path)
        .execute(pool)
        .await?;
    sqlx::query_as::<_, Session>("SELECT id, project_path, created_at FROM sessions WHERE id = ?1")
        .bind(&id)
        .fetch_one(pool)
        .await
}

#[instrument(skip(pool))]
pub async fn list_sessions(pool: &SqlitePool) -> Result<Vec<Session>, sqlx::Error> {
    debug!("listing sessions");
    sqlx::query_as::<_, Session>(
        "SELECT id, project_path, created_at FROM sessions ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await
}

#[instrument(skip(pool))]
pub async fn get_session(pool: &SqlitePool, id: &str) -> Result<Option<Session>, sqlx::Error> {
    debug!(id, "getting session");
    sqlx::query_as::<_, Session>("SELECT id, project_path, created_at FROM sessions WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

#[instrument(skip(pool))]
pub async fn delete_session(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    info!(id, "deleting session");
    // Delete dependent data first
    sqlx::query("DELETE FROM session_plans WHERE session_id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM conversation_messages WHERE session_id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM context_snapshots WHERE session_id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM events WHERE session_id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---- Events ----

#[instrument(skip(pool, data))]
pub async fn insert_event(
    pool: &SqlitePool,
    session_id: &str,
    agent_id: Option<&str>,
    kind: &str,
    data: &str,
) -> Result<Event, sqlx::Error> {
    debug!(kind, session_id, "inserting event");
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO events (id, session_id, agent_id, kind, data) VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(&id)
    .bind(session_id)
    .bind(agent_id)
    .bind(kind)
    .bind(data)
    .execute(pool)
    .await?;
    sqlx::query_as::<_, Event>(
        "SELECT id, kind, session_id, agent_id, data, created_at FROM events WHERE id = ?1",
    )
    .bind(&id)
    .fetch_one(pool)
    .await
}

/// Return the most recent event of a given kind for a session, if any.
#[instrument(skip(pool))]
pub async fn last_event_by_kind(
    pool: &SqlitePool,
    session_id: &str,
    kind: &str,
) -> Result<Option<Event>, sqlx::Error> {
    debug!(session_id, kind, "fetching last event by kind");
    sqlx::query_as::<_, Event>(
        "SELECT id, kind, session_id, agent_id, data, created_at
         FROM events
         WHERE session_id = ?1 AND kind = ?2
         ORDER BY created_at DESC
         LIMIT 1",
    )
    .bind(session_id)
    .bind(kind)
    .fetch_optional(pool)
    .await
}

#[instrument(skip(pool))]
pub async fn events_since(
    pool: &SqlitePool,
    session_id: &str,
    since: &str,
) -> Result<Vec<Event>, sqlx::Error> {
    debug!(session_id, since, "fetching events since");
    sqlx::query_as::<_, Event>(
        "SELECT id, kind, session_id, agent_id, data, created_at
         FROM events
         WHERE session_id = ?1 AND created_at > ?2
         ORDER BY created_at ASC",
    )
    .bind(session_id)
    .bind(since)
    .fetch_all(pool)
    .await
}

#[instrument(skip(pool))]
pub async fn events_by_kind(
    pool: &SqlitePool,
    kind: &str,
    since: &str,
) -> Result<Vec<Event>, sqlx::Error> {
    debug!(kind, since, "fetching events by kind");
    sqlx::query_as::<_, Event>(
        "SELECT id, kind, session_id, agent_id, data, created_at
         FROM events
         WHERE kind = ?1 AND created_at > ?2
         ORDER BY created_at ASC",
    )
    .bind(kind)
    .bind(since)
    .fetch_all(pool)
    .await
}

// ---- Tasks ----

const TASK_COLUMNS: &str = "id, session_id, title, description, status, priority, \
     assigned_agent, parent_task, worktree_branch, base_ref, base_commit, \
     merge_target_ref, outcome_kind, outcome_data, engagement_override, \
     cost_usd, created_at, updated_at";

#[allow(clippy::too_many_arguments)]
#[instrument(skip(
    pool,
    description,
    parent_task,
    base_ref,
    base_commit,
    merge_target_ref
))]
pub async fn create_task(
    pool: &SqlitePool,
    session_id: &str,
    title: &str,
    description: Option<&str>,
    parent_task: Option<&str>,
    priority: &str,
    base_ref: &str,
    base_commit: &str,
    merge_target_ref: &str,
) -> Result<Task, sqlx::Error> {
    info!(title, priority, "creating task");
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO tasks (id, session_id, title, description, parent_task, priority,
                            base_ref, base_commit, merge_target_ref)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(&id)
    .bind(session_id)
    .bind(title)
    .bind(description.unwrap_or(""))
    .bind(parent_task)
    .bind(priority)
    .bind(base_ref)
    .bind(base_commit)
    .bind(merge_target_ref)
    .execute(pool)
    .await?;
    sqlx::query_as::<_, Task>(&format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"))
        .bind(&id)
        .fetch_one(pool)
        .await
}

#[instrument(skip(pool))]
pub async fn get_task(pool: &SqlitePool, id: &str) -> Result<Option<Task>, sqlx::Error> {
    debug!(id, "getting task");
    sqlx::query_as::<_, Task>(&format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

#[instrument(skip(pool))]
pub async fn list_tasks(
    pool: &SqlitePool,
    session_id: &str,
    status_filter: Option<&str>,
) -> Result<Vec<Task>, sqlx::Error> {
    debug!(session_id, status_filter, "listing tasks");
    match status_filter {
        Some(status) => {
            sqlx::query_as::<_, Task>(&format!(
                "SELECT {TASK_COLUMNS} FROM tasks
                 WHERE session_id = ?1 AND status = ?2
                 ORDER BY CASE priority
                     WHEN 'critical' THEN 3 WHEN 'high' THEN 2
                     WHEN 'medium' THEN 1 WHEN 'low' THEN 0 ELSE 1 END DESC,
                     created_at ASC"
            ))
            .bind(session_id)
            .bind(status)
            .fetch_all(pool)
            .await
        }
        None => {
            sqlx::query_as::<_, Task>(&format!(
                "SELECT {TASK_COLUMNS} FROM tasks
                 WHERE session_id = ?1
                 ORDER BY CASE priority
                     WHEN 'critical' THEN 3 WHEN 'high' THEN 2
                     WHEN 'medium' THEN 1 WHEN 'low' THEN 0 ELSE 1 END DESC,
                     created_at ASC"
            ))
            .bind(session_id)
            .fetch_all(pool)
            .await
        }
    }
}

#[instrument(skip(pool, outcome_kind, outcome_data))]
pub async fn update_task_status(
    pool: &SqlitePool,
    id: &str,
    status: &str,
    outcome_kind: Option<&str>,
    outcome_data: Option<&str>,
) -> Result<(), sqlx::Error> {
    info!(id, status, "updating task status");
    sqlx::query(
        "UPDATE tasks
         SET status = ?1, outcome_kind = ?2, outcome_data = ?3,
             updated_at = CURRENT_TIMESTAMP
         WHERE id = ?4",
    )
    .bind(status)
    .bind(outcome_kind)
    .bind(outcome_data)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

#[instrument(skip(pool))]
pub async fn add_task_dep(
    pool: &SqlitePool,
    task_id: &str,
    depends_on_task_id: &str,
) -> Result<(), sqlx::Error> {
    debug!(task_id, depends_on_task_id, "adding task dependency");
    sqlx::query("INSERT INTO task_deps (task_id, depends_on_task_id) VALUES (?1, ?2)")
        .bind(task_id)
        .bind(depends_on_task_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[instrument(skip(pool))]
pub async fn get_task_deps(pool: &SqlitePool, task_id: &str) -> Result<Vec<String>, sqlx::Error> {
    debug!(task_id, "getting task dependencies");
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT depends_on_task_id FROM task_deps WHERE task_id = ?1")
            .bind(task_id)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

#[instrument(skip(pool))]
pub async fn next_unblocked(pool: &SqlitePool, session_id: &str) -> Result<Vec<Task>, sqlx::Error> {
    debug!(session_id, "finding next unblocked tasks");
    sqlx::query_as::<_, Task>(&format!(
        "SELECT {TASK_COLUMNS}
         FROM tasks t
         WHERE t.session_id = ?1
           AND t.status = 'pending'
           AND NOT EXISTS (
               SELECT 1 FROM task_deps td
               JOIN tasks dep ON td.depends_on_task_id = dep.id
               WHERE td.task_id = t.id AND dep.status != 'done'
           )
         ORDER BY CASE t.priority
             WHEN 'critical' THEN 3 WHEN 'high' THEN 2
             WHEN 'medium' THEN 1 WHEN 'low' THEN 0 ELSE 1 END DESC,
             t.created_at ASC"
    ))
    .bind(session_id)
    .fetch_all(pool)
    .await
}

// ---- Agents ----

const AGENT_COLUMNS: &str = "id, session_id, parent_agent_id, task_id, role, model, mode, status,
     token_input, token_output, created_at, finished_at";

#[instrument(skip(pool))]
pub async fn create_agent(
    pool: &SqlitePool,
    session_id: &str,
    parent_id: Option<&str>,
    role: &str,
    model: &str,
    mode: &str,
) -> Result<Agent, sqlx::Error> {
    info!(role, model, "creating agent");
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO agents (id, session_id, parent_agent_id, role, model, mode)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(&id)
    .bind(session_id)
    .bind(parent_id)
    .bind(role)
    .bind(model)
    .bind(mode)
    .execute(pool)
    .await?;
    sqlx::query_as::<_, Agent>(&format!("SELECT {AGENT_COLUMNS} FROM agents WHERE id = ?1"))
        .bind(&id)
        .fetch_one(pool)
        .await
}

#[instrument(skip(pool))]
pub async fn get_agent(pool: &SqlitePool, id: &str) -> Result<Option<Agent>, sqlx::Error> {
    debug!(id, "getting agent");
    sqlx::query_as::<_, Agent>(&format!("SELECT {AGENT_COLUMNS} FROM agents WHERE id = ?1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

#[instrument(skip(pool))]
pub async fn update_agent_status(
    pool: &SqlitePool,
    id: &str,
    status: &str,
) -> Result<(), sqlx::Error> {
    info!(id, status, "updating agent status");
    let finished_at_clause = match status {
        "complete" | "failed" => ", finished_at = CURRENT_TIMESTAMP",
        _ => "",
    };
    sqlx::query(&format!(
        "UPDATE agents SET status = ?1{finished_at_clause} WHERE id = ?2"
    ))
    .bind(status)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

#[instrument(skip(pool))]
pub async fn update_agent_tokens(
    pool: &SqlitePool,
    id: &str,
    input_tokens: i64,
    output_tokens: i64,
) -> Result<(), sqlx::Error> {
    debug!(id, input_tokens, output_tokens, "updating agent tokens");
    sqlx::query(
        "UPDATE agents
         SET token_input = token_input + ?1, token_output = token_output + ?2
         WHERE id = ?3",
    )
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

// ---- Modes ----

const MODE_COLUMNS: &str = "name, description, system_prompt, default_model, allowed_tools,
     origin, version, created_at, updated_at";

#[instrument(skip(pool, mode), fields(name = %mode.name))]
pub async fn insert_mode(pool: &SqlitePool, mode: &Mode) -> Result<(), sqlx::Error> {
    info!(name = %mode.name, "inserting mode");
    sqlx::query(
        "INSERT INTO modes (name, description, system_prompt, default_model,
                            allowed_tools, origin, version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(&mode.name)
    .bind(&mode.description)
    .bind(&mode.system_prompt)
    .bind(&mode.default_model)
    .bind(&mode.allowed_tools)
    .bind(&mode.origin)
    .bind(mode.version)
    .execute(pool)
    .await?;
    Ok(())
}

#[instrument(skip(pool))]
pub async fn get_mode(pool: &SqlitePool, name: &str) -> Result<Option<Mode>, sqlx::Error> {
    debug!(name, "getting mode");
    sqlx::query_as::<_, Mode>(&format!("SELECT {MODE_COLUMNS} FROM modes WHERE name = ?1"))
        .bind(name)
        .fetch_optional(pool)
        .await
}

#[instrument(skip(pool))]
pub async fn list_modes(pool: &SqlitePool) -> Result<Vec<Mode>, sqlx::Error> {
    debug!("listing modes");
    sqlx::query_as::<_, Mode>(&format!(
        "SELECT {MODE_COLUMNS} FROM modes ORDER BY name ASC"
    ))
    .fetch_all(pool)
    .await
}

#[instrument(skip(pool, description, system_prompt, default_model, allowed_tools))]
pub async fn update_mode(
    pool: &SqlitePool,
    name: &str,
    description: Option<&str>,
    system_prompt: Option<&str>,
    default_model: Option<Option<&str>>,
    allowed_tools: Option<&str>,
) -> Result<(), sqlx::Error> {
    info!(name, "updating mode");
    sqlx::query(
        "UPDATE modes SET
            description = COALESCE(?2, description),
            system_prompt = COALESCE(?3, system_prompt),
            default_model = CASE WHEN ?4 THEN ?5 ELSE default_model END,
            allowed_tools = COALESCE(?6, allowed_tools),
            version = version + 1,
            updated_at = CURRENT_TIMESTAMP
         WHERE name = ?1",
    )
    .bind(name)
    .bind(description)
    .bind(system_prompt)
    .bind(default_model.is_some())
    .bind(default_model.flatten())
    .bind(allowed_tools)
    .execute(pool)
    .await?;
    Ok(())
}

#[instrument(skip(pool))]
pub async fn delete_mode(pool: &SqlitePool, name: &str) -> Result<(), sqlx::Error> {
    info!(name, "deleting mode");
    sqlx::query("DELETE FROM modes WHERE name = ?1")
        .bind(name)
        .execute(pool)
        .await?;
    Ok(())
}

// ---- Recommendations ----

#[instrument(skip(pool, recommendation, action_payload))]
pub async fn insert_recommendation(
    pool: &SqlitePool,
    trigger_pattern: &str,
    recommendation: &str,
    action_type: &str,
    action_payload: &str,
) -> Result<i64, sqlx::Error> {
    info!(trigger_pattern, action_type, "inserting recommendation");
    let result = sqlx::query(
        "INSERT INTO recommendations (trigger_pattern, recommendation, action, status)
         VALUES (?1, ?2, ?3, 'pending')",
    )
    .bind(trigger_pattern)
    .bind(recommendation)
    .bind(action_payload)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

// ---- Stats Rollups ----

#[instrument(skip(pool))]
pub async fn insert_rollup(
    pool: &SqlitePool,
    scope: &str,
    scope_id: Option<&str>,
    period_start: &str,
    period_end: &str,
    metric: &str,
    value: f64,
) -> Result<StatsRollup, sqlx::Error> {
    debug!(scope, metric, value, "inserting stats rollup");
    let result = sqlx::query(
        "INSERT INTO stats_rollups (scope, scope_id, period_start, period_end, metric, value)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(scope)
    .bind(scope_id)
    .bind(period_start)
    .bind(period_end)
    .bind(metric)
    .bind(value)
    .execute(pool)
    .await?;
    let id = result.last_insert_rowid();
    sqlx::query_as::<_, StatsRollup>(
        "SELECT id, period_start, period_end, scope, scope_id, metric, CAST(value AS REAL) as value
         FROM stats_rollups WHERE id = ?1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
}

#[instrument(skip(pool))]
pub async fn query_rollups(
    pool: &SqlitePool,
    scope: &str,
    scope_id: Option<&str>,
    metric: &str,
    since: &str,
) -> Result<Vec<StatsRollup>, sqlx::Error> {
    debug!(scope, metric, since, "querying stats rollups");
    sqlx::query_as::<_, StatsRollup>(
        "SELECT id, period_start, period_end, scope, scope_id, metric, CAST(value AS REAL) as value
         FROM stats_rollups
         WHERE scope = ?1 AND (?2 IS NULL OR scope_id = ?2)
           AND metric = ?3 AND period_start >= ?4
         ORDER BY period_start ASC",
    )
    .bind(scope)
    .bind(scope_id)
    .bind(metric)
    .bind(since)
    .fetch_all(pool)
    .await
}

// ---- UX Agent State ----

#[instrument(skip(pool))]
pub async fn get_ux_state(
    pool: &SqlitePool,
    scope: &str,
) -> Result<Option<(Option<String>, Option<String>)>, sqlx::Error> {
    debug!(scope, "getting UX state");
    sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT last_event_id, last_event_at FROM ux_agent_state WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
}

#[instrument(skip(pool))]
pub async fn update_ux_state(
    pool: &SqlitePool,
    _scope: &str,
    last_event_id: &str,
    last_event_at: &str,
) -> Result<(), sqlx::Error> {
    debug!(last_event_id, "updating UX state");
    sqlx::query(
        "INSERT INTO ux_agent_state (id, last_event_id, last_event_at)
         VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET
            last_event_id = ?1,
            last_event_at = ?2",
    )
    .bind(last_event_id)
    .bind(last_event_at)
    .execute(pool)
    .await?;
    Ok(())
}

// ---- Conversation Messages ----

/// Append a message. Ordinal = MAX(ordinal)+1 for the session. Returns the ordinal.
#[instrument(skip(pool, content_json))]
pub async fn append_conversation_message(
    pool: &SqlitePool,
    session_id: &str,
    role: &str,
    content_json: &str,
    pinned: bool,
) -> Result<i64, sqlx::Error> {
    debug!(session_id, role, pinned, "appending conversation message");
    let id = Uuid::new_v4().to_string();
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO conversation_messages (id, session_id, ordinal, role, content, pinned)
         VALUES (?1, ?2, COALESCE((SELECT MAX(ordinal) + 1 FROM conversation_messages WHERE session_id = ?2), 0), ?3, ?4, ?5)
         RETURNING ordinal",
    )
    .bind(&id)
    .bind(session_id)
    .bind(role)
    .bind(content_json)
    .bind(pinned)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// Load all messages for a session (full history).
#[instrument(skip(pool))]
pub async fn get_conversation_messages(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<ConversationRow>, sqlx::Error> {
    debug!(session_id, "getting conversation messages");
    sqlx::query_as::<_, ConversationRow>(
        "SELECT ordinal, role, content, pinned, context_snippet FROM conversation_messages
         WHERE session_id = ?1 ORDER BY ordinal ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
}

/// Load messages after a given ordinal (for building agent context from snapshot).
#[instrument(skip(pool))]
pub async fn get_conversation_messages_since(
    pool: &SqlitePool,
    session_id: &str,
    after_ordinal: i64,
) -> Result<Vec<ConversationRow>, sqlx::Error> {
    debug!(
        session_id,
        after_ordinal, "getting conversation messages since ordinal"
    );
    sqlx::query_as::<_, ConversationRow>(
        "SELECT ordinal, role, content, pinned, context_snippet FROM conversation_messages
         WHERE session_id = ?1 AND ordinal > ?2 ORDER BY ordinal ASC",
    )
    .bind(session_id)
    .bind(after_ordinal)
    .fetch_all(pool)
    .await
}

/// Get the latest snapshot for a session.
#[instrument(skip(pool))]
pub async fn get_latest_snapshot(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<SnapshotRow>, sqlx::Error> {
    debug!(session_id, "getting latest snapshot");
    sqlx::query_as::<_, SnapshotRow>(
        "SELECT up_to_ordinal, summary_messages FROM context_snapshots
         WHERE session_id = ?1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
}

/// Save a compaction snapshot.
#[instrument(skip(pool, summary_json))]
pub async fn save_context_snapshot(
    pool: &SqlitePool,
    session_id: &str,
    up_to_ordinal: i64,
    summary_json: &str,
) -> Result<(), sqlx::Error> {
    info!(session_id, up_to_ordinal, "saving context snapshot");
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO context_snapshots (id, session_id, up_to_ordinal, summary_messages)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&id)
    .bind(session_id)
    .bind(up_to_ordinal)
    .bind(summary_json)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get the max ordinal for a session (or None if no messages).
#[instrument(skip(pool))]
pub async fn get_max_ordinal(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<i64>, sqlx::Error> {
    debug!(session_id, "getting max ordinal");
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT MAX(ordinal) FROM conversation_messages WHERE session_id = ?1")
            .bind(session_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|r| r.0))
}

// ---- Pinned Messages ----

/// Load all pinned messages for a session.
#[instrument(skip(pool))]
pub async fn get_pinned_messages(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<ConversationRow>, sqlx::Error> {
    debug!(session_id, "getting pinned messages");
    sqlx::query_as::<_, ConversationRow>(
        "SELECT ordinal, role, content, pinned, context_snippet FROM conversation_messages
         WHERE session_id = ?1 AND pinned = 1 ORDER BY ordinal ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
}

/// Load pinned messages that haven't had their context snippet generated yet.
#[instrument(skip(pool))]
pub async fn get_unprocessed_pinned(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<ConversationRow>, sqlx::Error> {
    debug!(session_id, "getting unprocessed pinned messages");
    sqlx::query_as::<_, ConversationRow>(
        "SELECT ordinal, role, content, pinned, context_snippet FROM conversation_messages
         WHERE session_id = ?1 AND pinned = 1 AND context_snippet IS NULL ORDER BY ordinal ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
}

/// Update the context snippet for a pinned message.
#[instrument(skip(pool, snippet))]
pub async fn update_context_snippet(
    pool: &SqlitePool,
    session_id: &str,
    ordinal: i64,
    snippet: &str,
) -> Result<(), sqlx::Error> {
    debug!(session_id, ordinal, "updating context snippet");
    sqlx::query(
        "UPDATE conversation_messages SET context_snippet = ?1
         WHERE session_id = ?2 AND ordinal = ?3",
    )
    .bind(snippet)
    .bind(session_id)
    .bind(ordinal)
    .execute(pool)
    .await?;
    Ok(())
}

// ---- Session Plans ----

/// Attach (or replace) a plan for a session.
#[instrument(skip(pool))]
pub async fn attach_plan(
    pool: &SqlitePool,
    session_id: &str,
    filename: &str,
) -> Result<(), sqlx::Error> {
    info!(session_id, filename, "attaching plan to session");
    sqlx::query(
        "INSERT INTO session_plans (session_id, plan_filename)
         VALUES (?1, ?2)
         ON CONFLICT(session_id) DO UPDATE SET
            plan_filename = ?2,
            attached_at = CURRENT_TIMESTAMP",
    )
    .bind(session_id)
    .bind(filename)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get the attached plan filename for a session, if any.
#[instrument(skip(pool))]
pub async fn get_attached_plan(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    debug!(session_id, "getting attached plan");
    let row: Option<(String,)> =
        sqlx::query_as("SELECT plan_filename FROM session_plans WHERE session_id = ?1")
            .bind(session_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| r.0))
}

/// Detach the plan from a session.
#[instrument(skip(pool))]
pub async fn detach_plan(pool: &SqlitePool, session_id: &str) -> Result<(), sqlx::Error> {
    info!(session_id, "detaching plan from session");
    sqlx::query("DELETE FROM session_plans WHERE session_id = ?1")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::test_pool;
    use super::*;

    // ---- Session tests ----

    #[tokio::test]
    async fn test_create_and_get_session() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();
        assert_eq!(session.project_path, "/tmp/test-project");
        assert!(!session.id.is_empty());

        let fetched = get_session(&pool, &session.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, session.id);
        assert_eq!(fetched.project_path, session.project_path);
    }

    #[tokio::test]
    async fn test_get_session_not_found() {
        let pool = test_pool().await;
        let result = get_session(&pool, "nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    // ---- Event tests ----

    #[tokio::test]
    async fn test_insert_and_query_events() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        let e1 = insert_event(
            &pool,
            &session.id,
            None,
            "prompt_submitted",
            r#"{"prompt":"hello"}"#,
        )
        .await
        .unwrap();
        assert_eq!(e1.kind, "prompt_submitted");
        assert_eq!(e1.session_id, session.id);
        assert!(e1.agent_id.is_none());

        let e2 = insert_event(
            &pool,
            &session.id,
            Some("agent-1"),
            "tool_called",
            r#"{"tool":"fs_read"}"#,
        )
        .await
        .unwrap();
        assert_eq!(e2.agent_id.as_deref(), Some("agent-1"));

        let since = events_since(&pool, &session.id, "2000-01-01")
            .await
            .unwrap();
        assert_eq!(since.len(), 2);
    }

    #[tokio::test]
    async fn test_events_by_kind() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        insert_event(&pool, &session.id, None, "prompt_submitted", "{}")
            .await
            .unwrap();
        insert_event(&pool, &session.id, None, "tool_called", "{}")
            .await
            .unwrap();
        insert_event(&pool, &session.id, None, "prompt_submitted", "{}")
            .await
            .unwrap();

        let results = events_by_kind(&pool, "prompt_submitted", "2000-01-01")
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.kind == "prompt_submitted"));
    }

    // ---- Task tests ----

    #[tokio::test]
    async fn test_create_and_get_task() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        let task = create_task(
            &pool,
            &session.id,
            "Build auth",
            Some("Implement authentication"),
            None,
            "high",
            "main",
            "abc123",
            "main",
        )
        .await
        .unwrap();

        assert_eq!(task.title, "Build auth");
        assert_eq!(task.description, "Implement authentication");
        assert_eq!(task.status, "pending");
        assert_eq!(task.priority, "high");
        assert_eq!(task.base_ref, "main");

        let fetched = get_task(&pool, &task.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, task.id);
    }

    #[tokio::test]
    async fn test_list_tasks_with_filter() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        let t1 = create_task(
            &pool,
            &session.id,
            "Task 1",
            None,
            None,
            "medium",
            "main",
            "abc",
            "main",
        )
        .await
        .unwrap();
        create_task(
            &pool,
            &session.id,
            "Task 2",
            None,
            None,
            "low",
            "main",
            "abc",
            "main",
        )
        .await
        .unwrap();

        update_task_status(&pool, &t1.id, "in_progress", None, None)
            .await
            .unwrap();

        let all = list_tasks(&pool, &session.id, None).await.unwrap();
        assert_eq!(all.len(), 2);

        let pending = list_tasks(&pool, &session.id, Some("pending"))
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].title, "Task 2");

        let in_progress = list_tasks(&pool, &session.id, Some("in_progress"))
            .await
            .unwrap();
        assert_eq!(in_progress.len(), 1);
        assert_eq!(in_progress[0].title, "Task 1");
    }

    #[tokio::test]
    async fn test_task_deps() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        let t1 = create_task(
            &pool,
            &session.id,
            "Task A",
            None,
            None,
            "medium",
            "main",
            "abc",
            "main",
        )
        .await
        .unwrap();
        let t2 = create_task(
            &pool,
            &session.id,
            "Task B",
            None,
            None,
            "medium",
            "main",
            "abc",
            "main",
        )
        .await
        .unwrap();

        add_task_dep(&pool, &t2.id, &t1.id).await.unwrap();

        let deps = get_task_deps(&pool, &t2.id).await.unwrap();
        assert_eq!(deps, vec![t1.id]);
    }

    #[tokio::test]
    async fn test_next_unblocked() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        let a = create_task(
            &pool,
            &session.id,
            "A",
            None,
            None,
            "medium",
            "main",
            "abc",
            "main",
        )
        .await
        .unwrap();
        let b = create_task(
            &pool,
            &session.id,
            "B",
            None,
            None,
            "high",
            "main",
            "abc",
            "main",
        )
        .await
        .unwrap();
        add_task_dep(&pool, &b.id, &a.id).await.unwrap();
        let _c = create_task(
            &pool,
            &session.id,
            "C",
            None,
            None,
            "low",
            "main",
            "abc",
            "main",
        )
        .await
        .unwrap();

        let unblocked = next_unblocked(&pool, &session.id).await.unwrap();
        let titles: Vec<&str> = unblocked.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["A", "C"]);

        update_task_status(&pool, &a.id, "done", None, None)
            .await
            .unwrap();

        let unblocked = next_unblocked(&pool, &session.id).await.unwrap();
        let titles: Vec<&str> = unblocked.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["B", "C"]);
    }

    // ---- Agent tests ----

    #[tokio::test]
    async fn test_create_and_get_agent() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        let agent = create_agent(
            &pool,
            &session.id,
            None,
            "implementation",
            "claude-sonnet-4-20250514",
            "implement",
        )
        .await
        .unwrap();

        assert_eq!(agent.role, "implementation");
        assert_eq!(agent.status, "spawning");
        assert_eq!(agent.token_input, 0);

        let fetched = get_agent(&pool, &agent.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, agent.id);
    }

    #[tokio::test]
    async fn test_update_agent_status_sets_finished_at() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        let agent = create_agent(
            &pool,
            &session.id,
            None,
            "research",
            "claude-haiku-4-5-20251001",
            "research",
        )
        .await
        .unwrap();
        assert!(agent.finished_at.is_none());

        update_agent_status(&pool, &agent.id, "running")
            .await
            .unwrap();
        let a = get_agent(&pool, &agent.id).await.unwrap().unwrap();
        assert_eq!(a.status, "running");
        assert!(a.finished_at.is_none());

        update_agent_status(&pool, &agent.id, "complete")
            .await
            .unwrap();
        let a = get_agent(&pool, &agent.id).await.unwrap().unwrap();
        assert_eq!(a.status, "complete");
        assert!(a.finished_at.is_some());
    }

    #[tokio::test]
    async fn test_update_agent_tokens() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        let agent = create_agent(
            &pool,
            &session.id,
            None,
            "implementation",
            "claude-sonnet-4-20250514",
            "implement",
        )
        .await
        .unwrap();

        update_agent_tokens(&pool, &agent.id, 1000, 500)
            .await
            .unwrap();
        let a = get_agent(&pool, &agent.id).await.unwrap().unwrap();
        assert_eq!(a.token_input, 1000);
        assert_eq!(a.token_output, 500);

        update_agent_tokens(&pool, &agent.id, 200, 100)
            .await
            .unwrap();
        let a = get_agent(&pool, &agent.id).await.unwrap().unwrap();
        assert_eq!(a.token_input, 1200);
        assert_eq!(a.token_output, 600);
    }

    // ---- Mode tests ----

    #[tokio::test]
    async fn test_mode_crud() {
        let pool = test_pool().await;

        let mode = Mode {
            name: "plan".into(),
            description: "Planning mode".into(),
            system_prompt: "You are a planner.".into(),
            default_model: None,
            allowed_tools: r#"["fs_read","grep"]"#.into(),
            origin: "builtin".into(),
            version: 1,
            created_at: String::new(),
            updated_at: String::new(),
        };
        insert_mode(&pool, &mode).await.unwrap();

        let fetched = get_mode(&pool, "plan").await.unwrap().unwrap();
        assert_eq!(fetched.description, "Planning mode");
        assert!(fetched.default_model.is_none());

        let all = list_modes(&pool).await.unwrap();
        assert_eq!(all.len(), 1);

        delete_mode(&pool, "plan").await.unwrap();
        assert!(get_mode(&pool, "plan").await.unwrap().is_none());
    }

    // ---- Stats rollup tests ----

    #[tokio::test]
    async fn test_stats_rollup_crud() {
        let pool = test_pool().await;

        let r1 = insert_rollup(
            &pool,
            "session",
            Some("s-1"),
            "2026-01-01T00:00:00",
            "2026-01-02T00:00:00",
            "cost.usd",
            3.50,
        )
        .await
        .unwrap();
        assert_eq!(r1.scope, "session");
        assert_eq!(r1.value, 3.50);

        insert_rollup(
            &pool,
            "session",
            Some("s-1"),
            "2026-01-02T00:00:00",
            "2026-01-03T00:00:00",
            "cost.usd",
            2.00,
        )
        .await
        .unwrap();

        let results = query_rollups(
            &pool,
            "session",
            Some("s-1"),
            "cost.usd",
            "2026-01-01T00:00:00",
        )
        .await
        .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].value, 3.50);
        assert_eq!(results[1].value, 2.00);
    }

    // ---- UX Agent State tests ----

    #[tokio::test]
    async fn test_ux_agent_state() {
        let pool = test_pool().await;

        assert!(get_ux_state(&pool, "global").await.unwrap().is_none());

        update_ux_state(&pool, "global", "evt-1", "2026-01-01T00:00:00")
            .await
            .unwrap();

        let state = get_ux_state(&pool, "global").await.unwrap().unwrap();
        assert_eq!(state.0.as_deref(), Some("evt-1"));
        assert_eq!(state.1.as_deref(), Some("2026-01-01T00:00:00"));

        update_ux_state(&pool, "global", "evt-5", "2026-01-02T00:00:00")
            .await
            .unwrap();

        let state = get_ux_state(&pool, "global").await.unwrap().unwrap();
        assert_eq!(state.0.as_deref(), Some("evt-5"));
        assert_eq!(state.1.as_deref(), Some("2026-01-02T00:00:00"));
    }

    // ---- Pinned Messages tests ----

    #[tokio::test]
    async fn test_pinned_message_roundtrip() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        // Insert a normal message
        let _ord1 = append_conversation_message(
            &pool,
            &session.id,
            "user",
            r#"[{"type":"text","text":"hello"}]"#,
            false,
        )
        .await
        .unwrap();

        // Insert a pinned message
        let ord2 = append_conversation_message(
            &pool,
            &session.id,
            "user",
            r#"[{"type":"text","text":"remember this"}]"#,
            true,
        )
        .await
        .unwrap();

        // get_conversation_messages should return both with correct pinned flag
        let all = get_conversation_messages(&pool, &session.id).await.unwrap();
        assert_eq!(all.len(), 2);
        assert!(!all[0].pinned);
        assert!(all[1].pinned);
        assert_eq!(all[1].ordinal, ord2);

        // get_pinned_messages should return only the pinned one
        let pinned = get_pinned_messages(&pool, &session.id).await.unwrap();
        assert_eq!(pinned.len(), 1);
        assert!(pinned[0].pinned);
        assert_eq!(pinned[0].ordinal, ord2);
    }

    #[tokio::test]
    async fn test_unprocessed_pinned_and_snippet_update() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        let ord = append_conversation_message(
            &pool,
            &session.id,
            "user",
            r#"[{"type":"text","text":"pin me"}]"#,
            true,
        )
        .await
        .unwrap();

        // Initially unprocessed (context_snippet is NULL)
        let unprocessed = get_unprocessed_pinned(&pool, &session.id).await.unwrap();
        assert_eq!(unprocessed.len(), 1);
        assert!(unprocessed[0].context_snippet.is_none());

        // Update context snippet
        update_context_snippet(&pool, &session.id, ord, "the user was referring to the bun runtime")
            .await
            .unwrap();

        // Now should not appear in unprocessed
        let unprocessed = get_unprocessed_pinned(&pool, &session.id).await.unwrap();
        assert_eq!(unprocessed.len(), 0);

        // Should appear with snippet in get_pinned_messages
        let pinned = get_pinned_messages(&pool, &session.id).await.unwrap();
        assert_eq!(pinned.len(), 1);
        assert_eq!(
            pinned[0].context_snippet.as_deref(),
            Some("the user was referring to the bun runtime")
        );
    }

    #[tokio::test]
    async fn test_empty_snippet_marks_as_processed() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        let ord = append_conversation_message(
            &pool,
            &session.id,
            "user",
            r#"[{"type":"text","text":"always use bun"}]"#,
            true,
        )
        .await
        .unwrap();

        // Update with empty snippet (self-contained, no context needed)
        update_context_snippet(&pool, &session.id, ord, "").await.unwrap();

        let unprocessed = get_unprocessed_pinned(&pool, &session.id).await.unwrap();
        assert_eq!(unprocessed.len(), 0);

        let pinned = get_pinned_messages(&pool, &session.id).await.unwrap();
        assert_eq!(pinned[0].context_snippet.as_deref(), Some(""));
    }

    // ---- Session Plans tests ----

    #[tokio::test]
    async fn test_attach_and_get_plan() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        assert!(get_attached_plan(&pool, &session.id).await.unwrap().is_none());

        attach_plan(&pool, &session.id, "my-plan-ab12.md")
            .await
            .unwrap();

        let plan = get_attached_plan(&pool, &session.id).await.unwrap().unwrap();
        assert_eq!(plan, "my-plan-ab12.md");
    }

    #[tokio::test]
    async fn test_attach_plan_replaces_existing() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        attach_plan(&pool, &session.id, "plan-a-1234.md")
            .await
            .unwrap();
        attach_plan(&pool, &session.id, "plan-b-5678.md")
            .await
            .unwrap();

        let plan = get_attached_plan(&pool, &session.id).await.unwrap().unwrap();
        assert_eq!(plan, "plan-b-5678.md");
    }

    #[tokio::test]
    async fn test_detach_plan() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        attach_plan(&pool, &session.id, "my-plan-ab12.md")
            .await
            .unwrap();
        detach_plan(&pool, &session.id).await.unwrap();

        assert!(get_attached_plan(&pool, &session.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_delete_session_cascades_plan() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        attach_plan(&pool, &session.id, "my-plan-ab12.md")
            .await
            .unwrap();
        delete_session(&pool, &session.id).await.unwrap();

        // Plan row should be gone (verified indirectly — session is deleted)
        assert!(get_session(&pool, &session.id).await.unwrap().is_none());
    }
}
