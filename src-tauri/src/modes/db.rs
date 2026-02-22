use super::types::{Mode, ModeOrigin};
use rusqlite::{params, Connection, Result as SqlResult};

#[derive(Debug, thiserror::Error)]
pub enum ModeError {
    #[error("cannot delete builtin mode: {0}")]
    CannotDeleteBuiltin(String),
    #[error("mode not found: {0}")]
    NotFound(String),
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
}

pub fn ensure_modes_table(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS modes (
            name TEXT PRIMARY KEY,
            description TEXT NOT NULL,
            system_prompt TEXT NOT NULL,
            default_model TEXT,
            allowed_tools TEXT NOT NULL,
            origin TEXT NOT NULL,
            version INTEGER NOT NULL DEFAULT 1,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );",
    )?;
    Ok(())
}

fn row_to_mode(row: &rusqlite::Row) -> SqlResult<Mode> {
    let tools_json: String = row.get("allowed_tools")?;
    let origin_str: String = row.get("origin")?;
    Ok(Mode {
        name: row.get("name")?,
        description: row.get("description")?,
        system_prompt: row.get("system_prompt")?,
        default_model: row.get("default_model")?,
        allowed_tools: serde_json::from_str(&tools_json).unwrap_or_default(),
        created_by: origin_str.parse::<ModeOrigin>().unwrap_or(ModeOrigin::User),
        version: row.get("version")?,
    })
}

pub fn list_modes(conn: &Connection) -> SqlResult<Vec<Mode>> {
    let mut stmt = conn.prepare("SELECT * FROM modes")?;
    let modes = stmt
        .query_map([], |row| row_to_mode(row))?
        .collect::<SqlResult<Vec<_>>>()?;
    Ok(modes)
}

pub fn get_mode(conn: &Connection, name: &str) -> SqlResult<Option<Mode>> {
    let mut stmt = conn.prepare("SELECT * FROM modes WHERE name = ?1")?;
    let mut rows = stmt.query_map(params![name], |row| row_to_mode(row))?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

pub fn create_mode(conn: &Connection, mode: &Mode) -> SqlResult<()> {
    let tools_json = serde_json::to_string(&mode.allowed_tools).unwrap();
    conn.execute(
        "INSERT INTO modes (name, description, system_prompt, default_model, allowed_tools, origin, version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            mode.name,
            mode.description,
            mode.system_prompt,
            mode.default_model,
            tools_json,
            mode.created_by.to_string(),
            mode.version,
        ],
    )?;
    Ok(())
}

pub fn update_mode(conn: &Connection, name: &str, mode: &Mode) -> Result<(), ModeError> {
    let tools_json = serde_json::to_string(&mode.allowed_tools).unwrap();
    let updated = conn.execute(
        "UPDATE modes SET description = ?1, system_prompt = ?2, default_model = ?3,
         allowed_tools = ?4, origin = ?5, version = version + 1,
         updated_at = CURRENT_TIMESTAMP
         WHERE name = ?6",
        params![
            mode.description,
            mode.system_prompt,
            mode.default_model,
            tools_json,
            mode.created_by.to_string(),
            name,
        ],
    )?;
    if updated == 0 {
        return Err(ModeError::NotFound(name.to_string()));
    }
    Ok(())
}

pub fn delete_mode(conn: &Connection, name: &str) -> Result<bool, ModeError> {
    let origin: Option<String> = conn
        .query_row(
            "SELECT origin FROM modes WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )
        .ok();

    match origin.as_deref() {
        None => Ok(false),
        Some("builtin") => Err(ModeError::CannotDeleteBuiltin(name.to_string())),
        Some(_) => {
            conn.execute("DELETE FROM modes WHERE name = ?1", params![name])?;
            Ok(true)
        }
    }
}

pub fn upsert_mode(conn: &Connection, mode: &Mode) -> SqlResult<()> {
    let tools_json = serde_json::to_string(&mode.allowed_tools).unwrap();
    conn.execute(
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
        params![
            mode.name,
            mode.description,
            mode.system_prompt,
            mode.default_model,
            tools_json,
            mode.created_by.to_string(),
            mode.version,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ensure_modes_table(&conn).unwrap();
        conn
    }

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

    #[test]
    fn create_and_get_mode() {
        let conn = setup();
        let mode = test_mode("test", ModeOrigin::User);
        create_mode(&conn, &mode).unwrap();

        let fetched = get_mode(&conn, "test").unwrap().unwrap();
        assert_eq!(fetched.name, "test");
        assert_eq!(fetched.description, "test description");
        assert_eq!(fetched.allowed_tools, vec!["fs_read", "grep"]);
        assert_eq!(fetched.created_by, ModeOrigin::User);
        assert_eq!(fetched.version, 1);
    }

    #[test]
    fn get_mode_not_found() {
        let conn = setup();
        assert!(get_mode(&conn, "nonexistent").unwrap().is_none());
    }

    #[test]
    fn list_modes_returns_all() {
        let conn = setup();
        create_mode(&conn, &test_mode("a", ModeOrigin::BuiltIn)).unwrap();
        create_mode(&conn, &test_mode("b", ModeOrigin::User)).unwrap();
        create_mode(&conn, &test_mode("c", ModeOrigin::UxAgent)).unwrap();

        let modes = list_modes(&conn).unwrap();
        assert_eq!(modes.len(), 3);
    }

    #[test]
    fn update_mode_increments_version() {
        let conn = setup();
        let mode = test_mode("updatable", ModeOrigin::User);
        create_mode(&conn, &mode).unwrap();

        let mut updated = mode.clone();
        updated.description = "updated description".into();
        update_mode(&conn, "updatable", &updated).unwrap();

        let fetched = get_mode(&conn, "updatable").unwrap().unwrap();
        assert_eq!(fetched.description, "updated description");
        assert_eq!(fetched.version, 2);
    }

    #[test]
    fn update_mode_not_found() {
        let conn = setup();
        let mode = test_mode("ghost", ModeOrigin::User);
        let err = update_mode(&conn, "ghost", &mode).unwrap_err();
        assert!(matches!(err, ModeError::NotFound(_)));
    }

    #[test]
    fn delete_builtin_mode_fails() {
        let conn = setup();
        create_mode(&conn, &test_mode("plan", ModeOrigin::BuiltIn)).unwrap();

        let err = delete_mode(&conn, "plan").unwrap_err();
        assert!(matches!(err, ModeError::CannotDeleteBuiltin(_)));

        // mode should still exist
        assert!(get_mode(&conn, "plan").unwrap().is_some());
    }

    #[test]
    fn delete_user_mode_succeeds() {
        let conn = setup();
        create_mode(&conn, &test_mode("custom", ModeOrigin::User)).unwrap();

        assert!(delete_mode(&conn, "custom").unwrap());
        assert!(get_mode(&conn, "custom").unwrap().is_none());
    }

    #[test]
    fn delete_nonexistent_returns_false() {
        let conn = setup();
        assert!(!delete_mode(&conn, "nope").unwrap());
    }

    #[test]
    fn upsert_inserts_new() {
        let conn = setup();
        let mode = test_mode("fresh", ModeOrigin::UxAgent);
        upsert_mode(&conn, &mode).unwrap();

        let fetched = get_mode(&conn, "fresh").unwrap().unwrap();
        assert_eq!(fetched.created_by, ModeOrigin::UxAgent);
        assert_eq!(fetched.version, 1);
    }

    #[test]
    fn upsert_updates_existing() {
        let conn = setup();
        let mode = test_mode("sync", ModeOrigin::BuiltIn);
        create_mode(&conn, &mode).unwrap();

        let mut updated = mode.clone();
        updated.description = "synced from file".into();
        upsert_mode(&conn, &updated).unwrap();

        let fetched = get_mode(&conn, "sync").unwrap().unwrap();
        assert_eq!(fetched.description, "synced from file");
        assert_eq!(fetched.version, 2);
    }

    #[test]
    fn create_duplicate_fails() {
        let conn = setup();
        let mode = test_mode("dup", ModeOrigin::User);
        create_mode(&conn, &mode).unwrap();
        assert!(create_mode(&conn, &mode).is_err());
    }

    #[test]
    fn default_model_none_roundtrip() {
        let conn = setup();
        let mut mode = test_mode("nomodel", ModeOrigin::User);
        mode.default_model = None;
        create_mode(&conn, &mode).unwrap();

        let fetched = get_mode(&conn, "nomodel").unwrap().unwrap();
        assert_eq!(fetched.default_model, None);
    }
}
