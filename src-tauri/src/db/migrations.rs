use rusqlite::Connection;

struct Migration {
    version: i32,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: MIGRATION_001,
}];

pub fn run_migrations(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS migrations (
            version INTEGER PRIMARY KEY,
            applied_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );",
    )?;

    let current_version: i32 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM migrations",
        [],
        |row| row.get(0),
    )?;

    let pending: Vec<&Migration> = MIGRATIONS
        .iter()
        .filter(|m| m.version > current_version)
        .collect();

    if pending.is_empty() {
        return Ok(());
    }

    let tx = conn.unchecked_transaction()?;
    for migration in pending {
        tx.execute_batch(migration.sql)?;
        tx.execute(
            "INSERT INTO migrations (version) VALUES (?1)",
            [migration.version],
        )?;
    }
    tx.commit()?;

    Ok(())
}

const MIGRATION_001: &str = "
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    project_path TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE events (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    session_id TEXT NOT NULL,
    agent_id TEXT,
    data TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_events_session_created ON events(session_id, created_at);
CREATE INDEX idx_events_kind ON events(kind);
CREATE INDEX idx_events_agent ON events(agent_id);

CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    parent_id TEXT REFERENCES tasks(id),
    title TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    priority INTEGER NOT NULL DEFAULT 2,
    worktree_branch TEXT,
    base_ref TEXT NOT NULL,
    base_commit TEXT NOT NULL,
    merge_target_ref TEXT NOT NULL,
    outcome_kind TEXT,
    outcome_data TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    completed_at DATETIME
);
CREATE INDEX idx_tasks_session_status ON tasks(session_id, status);
CREATE INDEX idx_tasks_parent ON tasks(parent_id);

CREATE TABLE task_deps (
    task_id TEXT NOT NULL REFERENCES tasks(id),
    depends_on TEXT NOT NULL REFERENCES tasks(id),
    PRIMARY KEY (task_id, depends_on)
);

CREATE TABLE agents (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    parent_agent_id TEXT REFERENCES agents(id),
    task_id TEXT REFERENCES tasks(id),
    role TEXT NOT NULL,
    model TEXT NOT NULL,
    mode TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'spawning',
    token_input INTEGER DEFAULT 0,
    token_output INTEGER DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    finished_at DATETIME
);
CREATE INDEX idx_agents_session ON agents(session_id);
CREATE INDEX idx_agents_parent ON agents(parent_agent_id);

CREATE TABLE modes (
    name TEXT PRIMARY KEY,
    description TEXT NOT NULL,
    system_prompt TEXT NOT NULL,
    default_model TEXT,
    allowed_tools TEXT NOT NULL,
    origin TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE recommendations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trigger_pattern TEXT NOT NULL,
    recommendation TEXT NOT NULL,
    action_type TEXT NOT NULL,
    action_payload TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    applied_at DATETIME,
    reverted_at DATETIME,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE stats_rollups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    period_start DATETIME NOT NULL,
    period_end DATETIME NOT NULL,
    scope TEXT NOT NULL,
    scope_id TEXT,
    metric TEXT NOT NULL,
    value REAL NOT NULL
);
CREATE INDEX idx_stats_scope_metric ON stats_rollups(scope, scope_id, metric);

CREATE TABLE ux_agent_state (
    scope TEXT PRIMARY KEY,
    last_event_id TEXT,
    last_event_at DATETIME,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
";

#[cfg(test)]
mod tests {
    use super::*;

    fn open_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn
    }

    #[test]
    fn creates_migrations_table() {
        let conn = open_conn();
        run_migrations(&conn).unwrap();

        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM migrations", [], |row| row.get(0))
            .unwrap();
        assert!(count > 0);
    }

    #[test]
    fn idempotent() {
        let conn = open_conn();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();

        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM migrations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, MIGRATIONS.len() as i32);
    }

    #[test]
    fn tracks_versions() {
        let conn = open_conn();
        run_migrations(&conn).unwrap();

        let versions: Vec<i32> = {
            let mut stmt = conn
                .prepare("SELECT version FROM migrations ORDER BY version")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        assert_eq!(versions, vec![1]);
    }

    #[test]
    fn records_applied_at() {
        let conn = open_conn();
        run_migrations(&conn).unwrap();

        let applied_at: String = conn
            .query_row(
                "SELECT applied_at FROM migrations WHERE version = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!applied_at.is_empty());
    }

    #[test]
    fn rolls_back_on_failure() {
        let conn = open_conn();
        run_migrations(&conn).unwrap();

        // Manually insert a future version to simulate partial state,
        // then try to apply a bad migration by temporarily swapping MIGRATIONS.
        // Instead, we test rollback by directly calling the transaction logic
        // with invalid SQL.
        let result: Result<(), rusqlite::Error> = (|| {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch("CREATE TABLE rollback_test (id INTEGER);")?;
            tx.execute_batch("INVALID SQL THAT WILL FAIL;")?;
            tx.commit()?;
            Ok(())
        })();

        assert!(result.is_err());

        // The rollback_test table should not exist
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='rollback_test'",
                [],
                |row| row.get::<_, i32>(0).map(|c| c > 0),
            )
            .unwrap();
        assert!(!table_exists);
    }
}
