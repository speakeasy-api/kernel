use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tauri::State;
use tracing::{debug, error, info, instrument};
use uuid::Uuid;

use super::decomposition::{DecompositionRequest, DecompositionResult};
use super::types::{EngagementLevel, Priority, Task, TaskOutcome, TaskStatus};

#[derive(Debug, Deserialize)]
pub struct CreateTaskInput {
    pub title: String,
    pub description: String,
    pub session_id: Uuid,
    pub parent_task: Option<Uuid>,
    pub depends_on: Vec<Uuid>,
    pub priority: Priority,
    pub base_ref: String,
    pub merge_target_ref: String,
    pub engagement_override: Option<EngagementLevel>,
}

#[derive(Debug, Serialize)]
pub struct TaskTree {
    pub tasks: Vec<Task>,
    pub edges: Vec<(Uuid, Uuid)>,
}

#[tauri::command]
#[instrument(skip(pool), fields(session_id = %input.session_id, title = %input.title, priority = ?input.priority))]
pub async fn create_task(
    pool: State<'_, SqlitePool>,
    input: CreateTaskInput,
) -> Result<Task, String> {
    let now = chrono::Utc::now();
    let task = Task {
        id: Uuid::new_v4(),
        session_id: input.session_id,
        title: input.title,
        description: input.description,
        status: TaskStatus::Pending,
        priority: input.priority,
        assigned_agent: None,
        parent_task: input.parent_task,
        depends_on: input.depends_on,
        worktree_branch: None,
        base_ref: input.base_ref,
        base_commit: String::new(),
        merge_target_ref: input.merge_target_ref,
        outcome: None,
        engagement_override: input.engagement_override,
        cost_usd: 0.0,
        created_at: now,
        updated_at: now,
    };
    info!(task_id = %task.id, "creating task");
    super::db::insert_task(&pool, &task).await.map_err(|e| {
        error!(error = %e, "create_task failed");
        e.to_string()
    })?;
    Ok(task)
}

#[tauri::command]
#[instrument(skip(pool, result), fields(session_id = %request.session_id))]
pub async fn persist_task_plan(
    pool: State<'_, SqlitePool>,
    request: DecompositionRequest,
    result: DecompositionResult,
) -> Result<Vec<Task>, String> {
    info!(task_count = result.tasks.len(), "persisting task plan");
    super::decomposition::persist_decomposition(&pool, &request, &result)
        .await
        .map_err(|e| {
            error!(error = %e, "persist_task_plan failed");
            e.to_string()
        })
}

#[tauri::command]
#[instrument(skip(pool))]
pub async fn list_tasks(
    pool: State<'_, SqlitePool>,
    session_id: Uuid,
    status_filter: Option<TaskStatus>,
) -> Result<Vec<Task>, String> {
    debug!(%session_id, ?status_filter, "listing tasks");
    super::db::list_tasks(&pool, session_id, status_filter)
        .await
        .map_err(|e| {
            error!(error = %e, "list_tasks failed");
            e.to_string()
        })
}

#[tauri::command]
#[instrument(skip(pool))]
pub async fn get_task(pool: State<'_, SqlitePool>, task_id: Uuid) -> Result<Option<Task>, String> {
    debug!(%task_id, "getting task");
    super::db::get_task(&pool, task_id).await.map_err(|e| {
        error!(error = %e, "get_task failed");
        e.to_string()
    })
}

#[tauri::command]
#[instrument(skip(pool))]
pub async fn update_task_status(
    pool: State<'_, SqlitePool>,
    task_id: Uuid,
    status: TaskStatus,
    outcome: Option<TaskOutcome>,
) -> Result<(), String> {
    info!(%task_id, ?status, "updating task status");
    super::lifecycle::apply_transition(&pool, task_id, status, outcome.as_ref())
        .await
        .map_err(|e| {
            error!(error = %e, %task_id, "update_task_status transition failed");
            e.to_string()
        })?;
    if status == TaskStatus::Done {
        super::lifecycle::cascade_unblock(&pool, task_id)
            .await
            .map_err(|e| {
                error!(error = %e, %task_id, "cascade_unblock failed");
                e.to_string()
            })?;
    }
    Ok(())
}

#[tauri::command]
#[instrument(skip(pool))]
pub async fn get_task_tree(
    pool: State<'_, SqlitePool>,
    session_id: Uuid,
) -> Result<TaskTree, String> {
    debug!(%session_id, "getting task tree");
    let (tasks, edges) = super::db::get_task_tree(&pool, session_id)
        .await
        .map_err(|e| {
            error!(error = %e, "get_task_tree failed");
            e.to_string()
        })?;
    Ok(TaskTree { tasks, edges })
}

#[tauri::command]
#[instrument(skip(pool))]
pub async fn next_unblocked(
    pool: State<'_, SqlitePool>,
    session_id: Uuid,
) -> Result<Vec<Task>, String> {
    debug!(%session_id, "querying next unblocked tasks");
    super::db::next_unblocked(&pool, session_id)
        .await
        .map_err(|e| {
            error!(error = %e, "next_unblocked failed");
            e.to_string()
        })
}

#[tauri::command]
#[instrument(skip(pool))]
pub async fn set_task_engagement(
    pool: State<'_, SqlitePool>,
    task_id: Uuid,
    engagement: Option<EngagementLevel>,
) -> Result<(), String> {
    info!(%task_id, ?engagement, "setting task engagement");
    super::db::update_task_engagement(&pool, task_id, engagement)
        .await
        .map_err(|e| {
            error!(error = %e, "set_task_engagement failed");
            e.to_string()
        })
}

#[tauri::command]
#[instrument(skip(pool))]
pub async fn get_task_cost(pool: State<'_, SqlitePool>, task_id: Uuid) -> Result<f64, String> {
    debug!(%task_id, "getting task cost");
    let row: (f64,) = sqlx::query_as("SELECT cost_usd FROM tasks WHERE id = ?1")
        .bind(task_id.to_string())
        .fetch_one(pool.inner())
        .await
        .map_err(|e| {
            error!(error = %e, "get_task_cost failed");
            e.to_string()
        })?;
    Ok(row.0)
}

#[tauri::command]
#[instrument(skip(pool))]
pub async fn get_session_cost(
    pool: State<'_, SqlitePool>,
    session_id: Uuid,
) -> Result<f64, String> {
    debug!(%session_id, "getting session cost");
    super::costs::session_cost(&pool, session_id)
        .await
        .map_err(|e| {
            error!(error = %e, "get_session_cost failed");
            e.to_string()
        })
}
