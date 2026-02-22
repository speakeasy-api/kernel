pub(crate) mod migrations;
pub mod models;
pub mod queries;
pub mod retention;

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

const KERNEL_DIR: &str = ".kernel";
const DB_FILE: &str = "kernel.db";

pub struct Database {
    conn: Connection,
    #[allow(dead_code)]
    path: PathBuf,
}

impl Database {
    pub fn open(project_root: &Path) -> Result<Self, rusqlite::Error> {
        let kernel_dir = project_root.join(KERNEL_DIR);
        fs::create_dir_all(&kernel_dir).map_err(|e| {
            rusqlite::Error::InvalidPath(
                kernel_dir.join(format!(" (failed to create directory: {e})")),
            )
        })?;

        let db_path = kernel_dir.join(DB_FILE);
        let conn = Connection::open(&db_path)?;

        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        migrations::run_migrations(&conn)?;

        let db = Self {
            conn,
            path: db_path,
        };

        Ok(db)
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_creates_kernel_dir_and_db() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::open(tmp.path()).unwrap();

        assert!(tmp.path().join(KERNEL_DIR).exists());
        assert!(tmp.path().join(KERNEL_DIR).join(DB_FILE).exists());

        // Verify WAL mode
        let mode: String = db
            .connection()
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");

        // Verify foreign keys
        let fk: i64 = db
            .connection()
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[test]
    fn test_migrations_are_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let _db1 = Database::open(tmp.path()).unwrap();
        // Opening again should not fail (migrations already applied)
        let _db2 = Database::open(tmp.path()).unwrap();
    }

    #[test]
    fn test_tables_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::open(tmp.path()).unwrap();

        let tables: Vec<String> = {
            let mut stmt = db
                .connection()
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };

        let expected = [
            "agents",
            "events",
            "migrations",
            "modes",
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
    }

    #[test]
    fn test_open_with_nonexistent_nested_path() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("deep").join("nested").join("project");
        let db = Database::open(&nested).unwrap();
        assert!(nested.join(KERNEL_DIR).join(DB_FILE).exists());
        drop(db);
    }
}
