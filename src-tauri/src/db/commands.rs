use sqlx::SqlitePool;
use tauri::State;
use tracing::{debug, error, info, instrument};

use super::models::{Event, Mode, Session};
use super::queries;

#[tauri::command]
#[instrument(skip(pool))]
pub async fn create_session(
    pool: State<'_, SqlitePool>,
    project_path: String,
) -> Result<Session, String> {
    info!(project_path, "cmd: create_session");
    queries::create_session(&pool, &project_path)
        .await
        .map_err(|e| { error!(error = %e, "create_session failed"); e.to_string() })
}

#[tauri::command]
#[instrument(skip(pool))]
pub async fn get_session(
    pool: State<'_, SqlitePool>,
    id: String,
) -> Result<Option<Session>, String> {
    debug!(id, "cmd: get_session");
    queries::get_session(&pool, &id)
        .await
        .map_err(|e| { error!(error = %e, "get_session failed"); e.to_string() })
}

#[tauri::command]
#[instrument(skip(pool))]
pub async fn list_sessions(pool: State<'_, SqlitePool>) -> Result<Vec<Session>, String> {
    debug!("cmd: list_sessions");
    queries::list_sessions(&pool)
        .await
        .map_err(|e| { error!(error = %e, "list_sessions failed"); e.to_string() })
}

#[tauri::command]
#[instrument(skip(pool))]
pub async fn delete_session(
    pool: State<'_, SqlitePool>,
    id: String,
) -> Result<(), String> {
    info!(id, "cmd: delete_session");
    queries::delete_session(&pool, &id)
        .await
        .map_err(|e| { error!(error = %e, "delete_session failed"); e.to_string() })
}

#[tauri::command]
#[instrument(skip(pool))]
pub async fn list_db_modes(pool: State<'_, SqlitePool>) -> Result<Vec<Mode>, String> {
    debug!("cmd: list_db_modes");
    queries::list_modes(&pool)
        .await
        .map_err(|e| { error!(error = %e, "list_db_modes failed"); e.to_string() })
}

#[tauri::command]
#[instrument(skip(pool))]
pub async fn get_db_mode(
    pool: State<'_, SqlitePool>,
    name: String,
) -> Result<Option<Mode>, String> {
    debug!(name, "cmd: get_db_mode");
    queries::get_mode(&pool, &name)
        .await
        .map_err(|e| { error!(error = %e, "get_db_mode failed"); e.to_string() })
}

#[tauri::command]
#[instrument(skip(pool, data))]
pub async fn insert_event(
    pool: State<'_, SqlitePool>,
    session_id: String,
    agent_id: Option<String>,
    kind: String,
    data: String,
) -> Result<Event, String> {
    debug!(kind, session_id, "cmd: insert_event");
    queries::insert_event(&pool, &session_id, agent_id.as_deref(), &kind, &data)
        .await
        .map_err(|e| { error!(error = %e, "insert_event failed"); e.to_string() })
}

#[tauri::command]
#[instrument(skip(pool))]
pub async fn events_since(
    pool: State<'_, SqlitePool>,
    session_id: String,
    since: String,
) -> Result<Vec<Event>, String> {
    debug!(session_id, since, "cmd: events_since");
    queries::events_since(&pool, &session_id, &since)
        .await
        .map_err(|e| { error!(error = %e, "events_since failed"); e.to_string() })
}
