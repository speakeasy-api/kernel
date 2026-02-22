use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use super::decomposition::{DecompositionRequest, DecompositionResult};
use super::types::{EngagementLevel, Priority, Task, TaskOutcome, TaskStatus};
use crate::DbState;

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
pub fn create_task(state: State<'_, DbState>, input: CreateTaskInput) -> Result<Task, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let now = Utc::now();
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
    super::db::insert_task(&conn, &task).map_err(|e| e.to_string())?;
    Ok(task)
}

#[tauri::command]
pub fn persist_task_plan(
    state: State<'_, DbState>,
    request: DecompositionRequest,
    result: DecompositionResult,
) -> Result<Vec<Task>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    super::decomposition::persist_decomposition(&conn, &request, &result).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_tasks(
    state: State<'_, DbState>,
    session_id: Uuid,
    status_filter: Option<TaskStatus>,
) -> Result<Vec<Task>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    super::db::list_tasks(&conn, session_id, status_filter).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_task(state: State<'_, DbState>, task_id: Uuid) -> Result<Option<Task>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    super::db::get_task(&conn, task_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_task_status(
    state: State<'_, DbState>,
    task_id: Uuid,
    status: TaskStatus,
    outcome: Option<TaskOutcome>,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    super::lifecycle::apply_transition(&conn, task_id, status, outcome.as_ref())
        .map_err(|e| e.to_string())?;
    if status == TaskStatus::Done {
        super::lifecycle::cascade_unblock(&conn, task_id).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn get_task_tree(state: State<'_, DbState>, session_id: Uuid) -> Result<TaskTree, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let (tasks, edges) = super::db::get_task_tree(&conn, session_id).map_err(|e| e.to_string())?;
    Ok(TaskTree { tasks, edges })
}

#[tauri::command]
pub fn next_unblocked(state: State<'_, DbState>, session_id: Uuid) -> Result<Vec<Task>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    super::db::next_unblocked(&conn, session_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_task_engagement(
    state: State<'_, DbState>,
    task_id: Uuid,
    engagement: Option<EngagementLevel>,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    super::db::update_task_engagement(&conn, task_id, engagement).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_task_cost(state: State<'_, DbState>, task_id: Uuid) -> Result<f64, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT cost_usd FROM tasks WHERE id = ?1",
        params![task_id.to_string()],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_session_cost(state: State<'_, DbState>, session_id: Uuid) -> Result<f64, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    super::costs::session_cost(&conn, session_id).map_err(|e| e.to_string())
}
