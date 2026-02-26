use sqlx::SqlitePool;
use tracing::{debug, error, info, instrument, warn};

use super::types::{Mode, ModeOrigin};

#[derive(Debug, thiserror::Error)]
pub enum ModeError {
    #[error("cannot delete builtin mode: {0}")]
    CannotDeleteBuiltin(String),
    #[error("mode not found: {0}")]
    NotFound(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

#[derive(sqlx::FromRow)]
struct ModeRow {
    name: String,
    description: String,
    system_prompt: String,
    default_model: Option<String>,
    allowed_tools: String,
    origin: String,
    version: i32,
}

fn row_to_mode(row: ModeRow) -> Mode {
    let allowed_tools: Vec<String> = serde_json::from_str(&row.allowed_tools).unwrap_or_default();
    let created_by = row.origin.parse::<ModeOrigin>().unwrap_or(ModeOrigin::User);
    Mode {
        name: row.name,
        description: row.description,
        system_prompt: row.system_prompt,
        default_model: row.default_model,
        allowed_tools,
        created_by,
        version: row.version as u32,
    }
}

#[instrument(skip(pool))]
pub async fn list_modes(pool: &SqlitePool) -> Result<Vec<Mode>, sqlx::Error> {
    debug!("listing all modes");
    let rows = sqlx::query_as::<_, ModeRow>(
        "SELECT name, description, system_prompt, default_model, allowed_tools, origin, version
         FROM modes",
    )
    .fetch_all(pool)
    .await?;
    debug!(count = rows.len(), "modes listed");
    Ok(rows.into_iter().map(row_to_mode).collect())
}

#[instrument(skip(pool))]
pub async fn get_mode(pool: &SqlitePool, name: &str) -> Result<Option<Mode>, sqlx::Error> {
    debug!(name, "getting mode");
    let row = sqlx::query_as::<_, ModeRow>(
        "SELECT name, description, system_prompt, default_model, allowed_tools, origin, version
         FROM modes WHERE name = ?1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await?;
    if row.is_none() {
        debug!(name, "mode not found");
    }
    Ok(row.map(row_to_mode))
}

#[instrument(skip(pool, mode), fields(name = %mode.name))]
pub async fn create_mode(pool: &SqlitePool, mode: &Mode) -> Result<(), sqlx::Error> {
    info!(name = %mode.name, origin = %mode.created_by, "creating mode");
    let tools_json = serde_json::to_string(&mode.allowed_tools).unwrap();
    sqlx::query(
        "INSERT INTO modes (name, description, system_prompt, default_model, allowed_tools, origin, version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(&mode.name)
    .bind(&mode.description)
    .bind(&mode.system_prompt)
    .bind(&mode.default_model)
    .bind(&tools_json)
    .bind(mode.created_by.to_string())
    .bind(mode.version)
    .execute(pool)
    .await?;
    Ok(())
}

#[instrument(skip(pool, mode), fields(name))]
pub async fn update_mode(pool: &SqlitePool, name: &str, mode: &Mode) -> Result<(), ModeError> {
    info!(name, "updating mode");
    let tools_json = serde_json::to_string(&mode.allowed_tools).unwrap();
    let result = sqlx::query(
        "UPDATE modes SET description = ?1, system_prompt = ?2, default_model = ?3,
         allowed_tools = ?4, origin = ?5, version = version + 1,
         updated_at = CURRENT_TIMESTAMP
         WHERE name = ?6",
    )
    .bind(&mode.description)
    .bind(&mode.system_prompt)
    .bind(&mode.default_model)
    .bind(&tools_json)
    .bind(mode.created_by.to_string())
    .bind(name)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        warn!(name, "mode not found for update");
        return Err(ModeError::NotFound(name.to_string()));
    }
    debug!(name, "mode updated");
    Ok(())
}

#[instrument(skip(pool))]
pub async fn delete_mode(pool: &SqlitePool, name: &str) -> Result<bool, ModeError> {
    let origin: Option<(String,)> = sqlx::query_as("SELECT origin FROM modes WHERE name = ?1")
        .bind(name)
        .fetch_optional(pool)
        .await?;

    match origin {
        None => {
            debug!(name, "mode not found for deletion");
            Ok(false)
        }
        Some((ref o,)) if o == "builtin" => {
            warn!(name, "attempted to delete builtin mode");
            Err(ModeError::CannotDeleteBuiltin(name.to_string()))
        }
        Some(_) => {
            sqlx::query("DELETE FROM modes WHERE name = ?1")
                .bind(name)
                .execute(pool)
                .await?;
            info!(name, "mode deleted");
            Ok(true)
        }
    }
}

#[instrument(skip(pool, mode), fields(name = %mode.name))]
pub async fn upsert_mode(pool: &SqlitePool, mode: &Mode) -> Result<(), sqlx::Error> {
    debug!(name = %mode.name, "upserting mode");
    let tools_json = serde_json::to_string(&mode.allowed_tools).unwrap();
    sqlx::query(
        "INSERT INTO modes (name, description, system_prompt, default_model, allowed_tools, origin, version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(name) DO UPDATE SET
            description = excluded.description,
            system_prompt = excluded.system_prompt,
            default_model = excluded.default_model,
            allowed_tools = excluded.allowed_tools,
            origin = excluded.origin,
            version = modes.version + 1,
            updated_at = CURRENT_TIMESTAMP",
    )
    .bind(&mode.name)
    .bind(&mode.description)
    .bind(&mode.system_prompt)
    .bind(&mode.default_model)
    .bind(&tools_json)
    .bind(mode.created_by.to_string())
    .bind(mode.version)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_pool;

    fn test_mode(name: &str, origin: ModeOrigin) -> Mode {
        Mode {
            name: name.to_string(),
            description: format!("{name} description"),
            system_prompt: format!("You are {name}."),
            default_model: Some("claude-sonnet-4-6".into()),
            allowed_tools: vec!["fs_read".into(), "grep".into()],
            created_by: origin,
            version: 1,
        }
    }

    #[tokio::test]
    async fn create_and_get_mode() {
        let pool = test_pool().await;
        let mode = test_mode("test", ModeOrigin::User);
        create_mode(&pool, &mode).await.unwrap();

        let fetched = get_mode(&pool, "test").await.unwrap().unwrap();
        assert_eq!(fetched.name, "test");
        assert_eq!(fetched.description, "test description");
        assert_eq!(fetched.allowed_tools, vec!["fs_read", "grep"]);
        assert_eq!(fetched.created_by, ModeOrigin::User);
        assert_eq!(fetched.version, 1);
    }

    #[tokio::test]
    async fn get_mode_not_found() {
        let pool = test_pool().await;
        assert!(get_mode(&pool, "nonexistent").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_modes_returns_all() {
        let pool = test_pool().await;
        create_mode(&pool, &test_mode("a", ModeOrigin::BuiltIn))
            .await
            .unwrap();
        create_mode(&pool, &test_mode("b", ModeOrigin::User))
            .await
            .unwrap();
        create_mode(&pool, &test_mode("c", ModeOrigin::UxAgent))
            .await
            .unwrap();

        let modes = list_modes(&pool).await.unwrap();
        assert_eq!(modes.len(), 3);
    }

    #[tokio::test]
    async fn update_mode_increments_version() {
        let pool = test_pool().await;
        let mode = test_mode("updatable", ModeOrigin::User);
        create_mode(&pool, &mode).await.unwrap();

        let mut updated = mode.clone();
        updated.description = "updated description".into();
        update_mode(&pool, "updatable", &updated).await.unwrap();

        let fetched = get_mode(&pool, "updatable").await.unwrap().unwrap();
        assert_eq!(fetched.description, "updated description");
        assert_eq!(fetched.version, 2);
    }

    #[tokio::test]
    async fn update_mode_not_found() {
        let pool = test_pool().await;
        let mode = test_mode("ghost", ModeOrigin::User);
        let err = update_mode(&pool, "ghost", &mode).await.unwrap_err();
        assert!(matches!(err, ModeError::NotFound(_)));
    }

    #[tokio::test]
    async fn delete_builtin_mode_fails() {
        let pool = test_pool().await;
        create_mode(&pool, &test_mode("plan", ModeOrigin::BuiltIn))
            .await
            .unwrap();

        let err = delete_mode(&pool, "plan").await.unwrap_err();
        assert!(matches!(err, ModeError::CannotDeleteBuiltin(_)));

        // mode should still exist
        assert!(get_mode(&pool, "plan").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn delete_user_mode_succeeds() {
        let pool = test_pool().await;
        create_mode(&pool, &test_mode("custom", ModeOrigin::User))
            .await
            .unwrap();

        assert!(delete_mode(&pool, "custom").await.unwrap());
        assert!(get_mode(&pool, "custom").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_false() {
        let pool = test_pool().await;
        assert!(!delete_mode(&pool, "nope").await.unwrap());
    }

    #[tokio::test]
    async fn upsert_inserts_new() {
        let pool = test_pool().await;
        let mode = test_mode("fresh", ModeOrigin::UxAgent);
        upsert_mode(&pool, &mode).await.unwrap();

        let fetched = get_mode(&pool, "fresh").await.unwrap().unwrap();
        assert_eq!(fetched.created_by, ModeOrigin::UxAgent);
        assert_eq!(fetched.version, 1);
    }

    #[tokio::test]
    async fn upsert_updates_existing() {
        let pool = test_pool().await;
        let mode = test_mode("sync", ModeOrigin::BuiltIn);
        create_mode(&pool, &mode).await.unwrap();

        let mut updated = mode.clone();
        updated.description = "synced from file".into();
        upsert_mode(&pool, &updated).await.unwrap();

        let fetched = get_mode(&pool, "sync").await.unwrap().unwrap();
        assert_eq!(fetched.description, "synced from file");
        assert_eq!(fetched.version, 2);
    }

    #[tokio::test]
    async fn create_duplicate_fails() {
        let pool = test_pool().await;
        let mode = test_mode("dup", ModeOrigin::User);
        create_mode(&pool, &mode).await.unwrap();
        assert!(create_mode(&pool, &mode).await.is_err());
    }

    #[tokio::test]
    async fn default_model_none_roundtrip() {
        let pool = test_pool().await;
        let mut mode = test_mode("nomodel", ModeOrigin::User);
        mode.default_model = None;
        create_mode(&pool, &mode).await.unwrap();

        let fetched = get_mode(&pool, "nomodel").await.unwrap().unwrap();
        assert_eq!(fetched.default_model, None);
    }
}
