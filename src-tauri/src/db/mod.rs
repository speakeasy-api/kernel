pub mod commands;
pub mod models;
pub mod queries;
pub mod retention;

use std::fs;
use std::path::{Path, PathBuf};

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

const KERNEL_DIR: &str = ".kernel";
const DB_FILE: &str = "kernel.db";

pub async fn create_pool(db_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let options: SqliteConnectOptions = db_url.parse::<SqliteConnectOptions>()?
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true)
        .create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    sqlx::migrate!().run(&pool).await?;

    Ok(pool)
}

pub fn kernel_data_dir() -> PathBuf {
    dirs::home_dir()
        .expect("could not determine home directory")
        .join(KERNEL_DIR)
}

pub async fn open_pool() -> Result<SqlitePool, sqlx::Error> {
    let kernel_dir = kernel_data_dir();
    fs::create_dir_all(&kernel_dir).map_err(|e| {
        sqlx::Error::Configuration(
            format!("failed to create {}: {e}", kernel_dir.display()).into(),
        )
    })?;

    let db_path = kernel_dir.join(DB_FILE);
    let db_url = format!("sqlite:{}", db_path.display());
    create_pool(&db_url).await
}

/// Open a pool at an arbitrary directory (used by tests).
#[cfg(test)]
async fn open_pool_at(root: &Path) -> Result<SqlitePool, sqlx::Error> {
    let kernel_dir = root.join(KERNEL_DIR);
    fs::create_dir_all(&kernel_dir).map_err(|e| {
        sqlx::Error::Configuration(
            format!("failed to create {}: {e}", kernel_dir.display()).into(),
        )
    })?;
    let db_path = kernel_dir.join(DB_FILE);
    let db_url = format!("sqlite:{}", db_path.display());
    create_pool(&db_url).await
}

#[cfg(test)]
pub async fn test_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("failed to create test pool");

    sqlx::migrate!().run(&pool).await.expect("failed to run migrations");

    pool
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_open_creates_kernel_dir_and_db() {
        let tmp = tempfile::tempdir().unwrap();
        let pool = open_pool_at(tmp.path()).await.unwrap();

        assert!(tmp.path().join(KERNEL_DIR).exists());
        assert!(tmp.path().join(KERNEL_DIR).join(DB_FILE).exists());

        // Verify WAL mode
        let row: (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.0.to_lowercase(), "wal");

        // Verify foreign keys
        let row: (i64,) = sqlx::query_as("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.0, 1);

        pool.close().await;
    }

    #[tokio::test]
    async fn test_migrations_are_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let pool1 = open_pool_at(tmp.path()).await.unwrap();
        pool1.close().await;
        // Opening again should not fail (migrations already applied)
        let pool2 = open_pool_at(tmp.path()).await.unwrap();
        pool2.close().await;
    }

    #[tokio::test]
    async fn test_tables_exist() {
        let pool = test_pool().await;

        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        let tables: Vec<String> = rows.into_iter().map(|r| r.0).collect();

        let expected = [
            "agents",
            "context_snapshots",
            "conversation_messages",
            "conventions",
            "corrections",
            "events",
            "modes",
            "recommendation_versions",
            "recommendations",
            "sessions",
            "stats_rollups",
            "task_deps",
            "tasks",
            "ux_agent_state",
        ];
        for table in &expected {
            assert!(
                tables.contains(&table.to_string()),
                "missing table: {table}"
            );
        }

        pool.close().await;
    }

    #[tokio::test]
    async fn test_open_with_nonexistent_nested_path() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("deep").join("nested").join("project");
        let pool = open_pool_at(&nested).await.unwrap();
        assert!(nested.join(KERNEL_DIR).join(DB_FILE).exists());
        pool.close().await;
    }
}
