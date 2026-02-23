use sqlx::SqlitePool;
use tauri::State;

use super::models::{Event, Mode, Session};
use super::queries;

#[tauri::command]
pub async fn create_session(
    pool: State<'_, SqlitePool>,
    project_path: String,
) -> Result<Session, String> {
    queries::create_session(&pool, &project_path)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_session(
    pool: State<'_, SqlitePool>,
    id: String,
) -> Result<Option<Session>, String> {
    queries::get_session(&pool, &id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_sessions(pool: State<'_, SqlitePool>) -> Result<Vec<Session>, String> {
    queries::list_sessions(&pool)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_session(
    pool: State<'_, SqlitePool>,
    id: String,
) -> Result<(), String> {
    queries::delete_session(&pool, &id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_db_modes(pool: State<'_, SqlitePool>) -> Result<Vec<Mode>, String> {
    queries::list_modes(&pool)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_db_mode(
    pool: State<'_, SqlitePool>,
    name: String,
) -> Result<Option<Mode>, String> {
    queries::get_mode(&pool, &name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn insert_event(
    pool: State<'_, SqlitePool>,
    session_id: String,
    agent_id: Option<String>,
    kind: String,
    data: String,
) -> Result<Event, String> {
    queries::insert_event(&pool, &session_id, agent_id.as_deref(), &kind, &data)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn events_since(
    pool: State<'_, SqlitePool>,
    session_id: String,
    since: String,
) -> Result<Vec<Event>, String> {
    queries::events_since(&pool, &session_id, &since)
        .await
        .map_err(|e| e.to_string())
}
