use std::error::Error;
use std::str::FromStr;

use rusqlite::{params, Connection, OptionalExtension};

use super::learning::{Convention, Correction, CorrectionType};
use super::types::{
    Recommendation, RecommendationAction, RecommendationStatus, RecommendationVersion,
    UxAgentState,
};

pub type StoreResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const UX_AGENT_SCOPE: &str = "ux_agent";

pub struct RecommendationStore<'conn> {
    conn: &'conn Connection,
}

impl<'conn> RecommendationStore<'conn> {
    pub fn new(conn: &'conn Connection) -> Self {
        Self { conn }
    }

    pub fn ensure_tables(conn: &Connection) -> StoreResult<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS recommendations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trigger_pattern TEXT NOT NULL,
                recommendation TEXT NOT NULL,
                action TEXT NOT NULL,
                status TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS ux_agent_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                last_event_id TEXT,
                last_event_at TEXT,
                last_run_at TEXT
            );
            CREATE TABLE IF NOT EXISTS recommendation_versions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                recommendation_id INTEGER NOT NULL,
                version INTEGER NOT NULL,
                applied_at TEXT NOT NULL,
                reverted_at TEXT,
                snapshot TEXT NOT NULL,
                FOREIGN KEY (recommendation_id) REFERENCES recommendations(id)
            );
            CREATE TABLE IF NOT EXISTS corrections (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT,
                correction_type TEXT NOT NULL,
                original_value TEXT,
                corrected_value TEXT,
                context TEXT,
                created_at TEXT DEFAULT (datetime('now')),
                incorporated INTEGER DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS conventions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                convention TEXT NOT NULL,
                source_corrections TEXT NOT NULL,
                target_mode TEXT,
                status TEXT DEFAULT 'proposed',
                created_at TEXT DEFAULT (datetime('now'))
            );",
        )?;
        if !has_column(conn, "ux_agent_state", "last_run_at")? {
            conn.execute("ALTER TABLE ux_agent_state ADD COLUMN last_run_at TEXT", [])?;
        }
        Ok(())
    }

    pub fn insert(&self, rec: &Recommendation) -> StoreResult<u64> {
        let action_json = serde_json::to_string(&rec.action)?;
        match self.recommendation_schema()? {
            RecommendationSchema::JsonAction => {
                self.conn.execute(
                    "INSERT INTO recommendations (trigger_pattern, recommendation, action, status)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        rec.trigger_pattern,
                        rec.recommendation,
                        action_json,
                        rec.status.to_string()
                    ],
                )?;
            }
            RecommendationSchema::LegacyActionPayload => {
                self.conn.execute(
                    "INSERT INTO recommendations (trigger_pattern, recommendation, action_type, action_payload, status)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        rec.trigger_pattern,
                        rec.recommendation,
                        action_type(&rec.action),
                        action_json,
                        rec.status.to_string()
                    ],
                )?;
            }
        }
        let id = u64::try_from(self.conn.last_insert_rowid())?;
        Ok(id)
    }

    pub fn get(&self, id: u64) -> StoreResult<Option<Recommendation>> {
        let id_i64 = i64::try_from(id)?;
        let rec = self
            .conn
            .query_row(
                &format!(
                    "SELECT id, trigger_pattern, recommendation, {}, status
                     FROM recommendations WHERE id = ?1",
                    self.action_column_expr()?
                ),
                params![id_i64],
                row_to_recommendation,
            )
            .optional()?;
        Ok(rec)
    }

    pub fn list_pending(&self) -> StoreResult<Vec<Recommendation>> {
        self.list_by_status(RecommendationStatus::Pending)
    }

    pub fn list_all(&self) -> StoreResult<Vec<Recommendation>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, trigger_pattern, recommendation, {}, status
             FROM recommendations
             ORDER BY id ASC",
            self.action_column_expr()?
        ))?;
        let rows = stmt.query_map([], row_to_recommendation)?;
        let recs = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(recs)
    }

    pub fn update_status(&self, id: u64, status: RecommendationStatus) -> StoreResult<()> {
        let id_i64 = i64::try_from(id)?;
        self.conn.execute(
            "UPDATE recommendations SET status = ?1 WHERE id = ?2",
            params![status.to_string(), id_i64],
        )?;
        Ok(())
    }

    pub fn get_cursor(&self) -> StoreResult<UxAgentState> {
        let state = match self.cursor_schema()? {
            CursorSchema::SingleRowId => self
                .conn
                .query_row(
                    "SELECT last_event_id, last_event_at, last_run_at
                     FROM ux_agent_state WHERE id = 1",
                    [],
                    row_to_cursor_state,
                )
                .optional()?,
            CursorSchema::Scoped => self
                .conn
                .query_row(
                    "SELECT last_event_id, last_event_at, last_run_at
                     FROM ux_agent_state WHERE scope = ?1",
                    params![UX_AGENT_SCOPE],
                    row_to_cursor_state,
                )
                .optional()?,
        };
        Ok(state.unwrap_or_default())
    }

    pub fn set_cursor(&self, state: &UxAgentState) -> StoreResult<()> {
        match self.cursor_schema()? {
            CursorSchema::SingleRowId => {
                self.conn.execute(
                    "INSERT INTO ux_agent_state (id, last_event_id, last_event_at, last_run_at)
                     VALUES (1, ?1, ?2, ?3)
                     ON CONFLICT(id) DO UPDATE SET
                        last_event_id = excluded.last_event_id,
                        last_event_at = excluded.last_event_at,
                        last_run_at = excluded.last_run_at",
                    params![state.last_event_id, state.last_event_at, state.last_run_at],
                )?;
            }
            CursorSchema::Scoped => {
                self.conn.execute(
                    "INSERT INTO ux_agent_state (scope, last_event_id, last_event_at, last_run_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
                     ON CONFLICT(scope) DO UPDATE SET
                        last_event_id = excluded.last_event_id,
                        last_event_at = excluded.last_event_at,
                        last_run_at = excluded.last_run_at,
                        updated_at = CURRENT_TIMESTAMP",
                    params![
                        UX_AGENT_SCOPE,
                        state.last_event_id,
                        state.last_event_at,
                        state.last_run_at
                    ],
                )?;
            }
        }
        Ok(())
    }

    pub fn insert_version(
        &self,
        recommendation_id: u64,
        version: u32,
        snapshot: &str,
    ) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO recommendation_versions (recommendation_id, version, applied_at, snapshot)
             VALUES (?1, ?2, CURRENT_TIMESTAMP, ?3)",
            params![i64::try_from(recommendation_id)?, version, snapshot],
        )?;
        Ok(())
    }

    pub fn get_versions(
        &self,
        recommendation_id: u64,
    ) -> StoreResult<Vec<RecommendationVersion>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, recommendation_id, version, applied_at, reverted_at, snapshot
             FROM recommendation_versions
             WHERE recommendation_id = ?1
             ORDER BY version ASC",
        )?;
        let rows = stmt.query_map(
            params![i64::try_from(recommendation_id)?],
            row_to_version,
        )?;
        let versions = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(versions)
    }

    pub fn mark_version_reverted(&self, version_id: u64) -> StoreResult<()> {
        self.conn.execute(
            "UPDATE recommendation_versions SET reverted_at = CURRENT_TIMESTAMP WHERE id = ?1",
            params![i64::try_from(version_id)?],
        )?;
        Ok(())
    }

    pub fn get_dismissed_patterns(&self) -> StoreResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT trigger_pattern FROM recommendations WHERE lower(status) = 'dismissed'",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let patterns = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(patterns)
    }

    // --- Corrections ---

    pub fn insert_correction(&self, correction: &Correction) -> StoreResult<u64> {
        self.conn.execute(
            "INSERT INTO corrections (session_id, correction_type, original_value, corrected_value, context, incorporated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                correction.session_id,
                correction.correction_type.as_str(),
                correction.original_value,
                correction.corrected_value,
                correction.context,
                correction.incorporated as i32,
            ],
        )?;
        let id = u64::try_from(self.conn.last_insert_rowid())?;
        Ok(id)
    }

    pub fn get_unincorporated_corrections(&self) -> StoreResult<Vec<Correction>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, correction_type, original_value, corrected_value, context, created_at, incorporated
             FROM corrections
             WHERE incorporated = 0
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], row_to_correction)?;
        let corrections = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(corrections)
    }

    pub fn get_corrections_by_type(
        &self,
        correction_type: &str,
    ) -> StoreResult<Vec<Correction>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, correction_type, original_value, corrected_value, context, created_at, incorporated
             FROM corrections
             WHERE correction_type = ?1
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![correction_type], row_to_correction)?;
        let corrections = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(corrections)
    }

    pub fn mark_corrections_incorporated(&self, ids: &[u64]) -> StoreResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "UPDATE corrections SET incorporated = 1 WHERE id IN ({})",
            placeholders.join(", ")
        );
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
            .iter()
            .map(|id| Box::new(i64::try_from(*id).unwrap_or(0)) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        self.conn.execute(&sql, param_refs.as_slice())?;
        Ok(())
    }

    // --- Conventions ---

    pub fn insert_convention(
        &self,
        convention: &str,
        source_ids: &[u64],
        target_mode: Option<&str>,
    ) -> StoreResult<u64> {
        let source_json = serde_json::to_string(source_ids)?;
        self.conn.execute(
            "INSERT INTO conventions (convention, source_corrections, target_mode)
             VALUES (?1, ?2, ?3)",
            params![convention, source_json, target_mode],
        )?;
        let id = u64::try_from(self.conn.last_insert_rowid())?;
        Ok(id)
    }

    pub fn get_proposed_conventions(&self) -> StoreResult<Vec<Convention>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, convention, source_corrections, target_mode, status, created_at
             FROM conventions
             WHERE status = 'proposed'
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], row_to_convention)?;
        let conventions = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(conventions)
    }

    pub fn update_convention_status(&self, id: u64, status: &str) -> StoreResult<()> {
        let id_i64 = i64::try_from(id)?;
        self.conn.execute(
            "UPDATE conventions SET status = ?1 WHERE id = ?2",
            params![status, id_i64],
        )?;
        Ok(())
    }

    fn list_by_status(&self, status: RecommendationStatus) -> StoreResult<Vec<Recommendation>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, trigger_pattern, recommendation, {}, status
             FROM recommendations
             WHERE lower(status) = lower(?1)
             ORDER BY id ASC",
            self.action_column_expr()?
        ))?;
        let rows = stmt.query_map(params![status.to_string()], row_to_recommendation)?;
        let recs = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(recs)
    }

    fn recommendation_schema(&self) -> StoreResult<RecommendationSchema> {
        if has_column(self.conn, "recommendations", "action")? {
            return Ok(RecommendationSchema::JsonAction);
        }
        if has_column(self.conn, "recommendations", "action_payload")? {
            return Ok(RecommendationSchema::LegacyActionPayload);
        }
        Err(store_error(
            "recommendations table is missing action columns",
        ))
    }

    fn action_column_expr(&self) -> StoreResult<&'static str> {
        let column = match self.recommendation_schema()? {
            RecommendationSchema::JsonAction => "action",
            RecommendationSchema::LegacyActionPayload => "action_payload",
        };
        Ok(column)
    }

    fn cursor_schema(&self) -> StoreResult<CursorSchema> {
        if has_column(self.conn, "ux_agent_state", "id")? {
            return Ok(CursorSchema::SingleRowId);
        }
        if has_column(self.conn, "ux_agent_state", "scope")? {
            return Ok(CursorSchema::Scoped);
        }
        Err(store_error(
            "ux_agent_state table is missing identifier columns",
        ))
    }
}

#[derive(Clone, Copy)]
enum RecommendationSchema {
    JsonAction,
    LegacyActionPayload,
}

#[derive(Clone, Copy)]
enum CursorSchema {
    SingleRowId,
    Scoped,
}

fn action_type(action: &RecommendationAction) -> &'static str {
    match action {
        RecommendationAction::ModelChange { .. } => "model_change",
        RecommendationAction::PromptEdit { .. } => "prompt_edit",
        RecommendationAction::ModeCreate { .. } => "mode_create",
        RecommendationAction::ModeEdit { .. } => "mode_edit",
        RecommendationAction::ConfigChange { .. } => "config_change",
    }
}

fn row_to_cursor_state(row: &rusqlite::Row<'_>) -> Result<UxAgentState, rusqlite::Error> {
    Ok(UxAgentState {
        last_event_id: row.get(0)?,
        last_event_at: row.get(1)?,
        last_run_at: row.get(2)?,
    })
}

fn has_column(conn: &Connection, table: &str, column: &str) -> StoreResult<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row?.as_str() == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn store_error(msg: &str) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        msg.to_string(),
    ))
}

fn row_to_version(row: &rusqlite::Row<'_>) -> Result<RecommendationVersion, rusqlite::Error> {
    Ok(RecommendationVersion {
        id: row.get(0)?,
        recommendation_id: row.get(1)?,
        version: row.get(2)?,
        applied_at: row.get(3)?,
        reverted_at: row.get(4)?,
        snapshot: row.get(5)?,
    })
}

fn row_to_correction(row: &rusqlite::Row<'_>) -> Result<Correction, rusqlite::Error> {
    let correction_type_str: String = row.get(2)?;
    let correction_type = CorrectionType::from_str(&correction_type_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown correction type: {correction_type_str}"),
            )),
        )
    })?;
    let incorporated_int: i32 = row.get(7)?;
    Ok(Correction {
        id: row.get(0)?,
        session_id: row.get(1)?,
        correction_type,
        original_value: row.get(3)?,
        corrected_value: row.get(4)?,
        context: row.get(5)?,
        created_at: row.get(6)?,
        incorporated: incorporated_int != 0,
    })
}

fn row_to_convention(row: &rusqlite::Row<'_>) -> Result<Convention, rusqlite::Error> {
    let source_json: String = row.get(2)?;
    let source_corrections: Vec<u64> =
        serde_json::from_str(&source_json).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
        })?;
    Ok(Convention {
        id: row.get(0)?,
        convention: row.get(1)?,
        source_corrections,
        target_mode: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn row_to_recommendation(row: &rusqlite::Row<'_>) -> Result<Recommendation, rusqlite::Error> {
    let id = row.get::<_, u64>(0)?;
    let action_json: String = row.get(3)?;
    let status_str: String = row.get(4)?;
    let action: RecommendationAction = serde_json::from_str(&action_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let status = RecommendationStatus::from_str(&status_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
        )
    })?;

    Ok(Recommendation {
        id,
        trigger_pattern: row.get(1)?,
        recommendation: row.get(2)?,
        action,
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        RecommendationStore::ensure_tables(&conn).unwrap();
        conn
    }

    fn sample_recommendation() -> Recommendation {
        Recommendation {
            id: 0,
            trigger_pattern: "3+ diff rejections on schema edits".to_string(),
            recommendation: "Switch planner model for schema tasks".to_string(),
            action: RecommendationAction::ModelChange {
                role: "planner".to_string(),
                from_model: "cheap-model".to_string(),
                to_model: "smarter-model".to_string(),
            },
            status: RecommendationStatus::Pending,
        }
    }

    #[test]
    fn recommendation_action_tagged_serde_roundtrip() {
        let action = RecommendationAction::ConfigChange {
            key: "ux_agent.interval_seconds".to_string(),
            old_value: "120".to_string(),
            new_value: "300".to_string(),
        };
        let encoded = serde_json::to_string(&action).unwrap();
        assert!(encoded.contains(r#""type":"ConfigChange""#));

        let decoded: RecommendationAction = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, action);
    }

    #[test]
    fn insert_and_get_recommendation() {
        let conn = setup();
        let store = RecommendationStore::new(&conn);
        let rec = sample_recommendation();

        let id = store.insert(&rec).unwrap();
        let loaded = store.get(id).unwrap().unwrap();

        assert_eq!(loaded.id, id);
        assert_eq!(loaded.trigger_pattern, rec.trigger_pattern);
        assert_eq!(loaded.recommendation, rec.recommendation);
        assert_eq!(loaded.action, rec.action);
        assert_eq!(loaded.status, RecommendationStatus::Pending);
    }

    #[test]
    fn list_pending_and_update_status() {
        let conn = setup();
        let store = RecommendationStore::new(&conn);
        let rec = sample_recommendation();
        let id = store.insert(&rec).unwrap();

        let pending = store.list_pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id);

        store
            .update_status(id, RecommendationStatus::Applied)
            .unwrap();

        let pending_after = store.list_pending().unwrap();
        assert!(pending_after.is_empty());

        let loaded = store.get(id).unwrap().unwrap();
        assert_eq!(loaded.status, RecommendationStatus::Applied);
    }

    #[test]
    fn list_all_returns_all_recommendations() {
        let conn = setup();
        let store = RecommendationStore::new(&conn);
        let rec = sample_recommendation();
        let first = store.insert(&rec).unwrap();
        let second = store.insert(&rec).unwrap();

        let all = store.list_all().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, first);
        assert_eq!(all[1].id, second);
    }

    #[test]
    fn cursor_defaults_when_empty() {
        let conn = setup();
        let store = RecommendationStore::new(&conn);
        let state = store.get_cursor().unwrap();
        assert_eq!(state, UxAgentState::default());
    }

    #[test]
    fn cursor_roundtrip() {
        let conn = setup();
        let store = RecommendationStore::new(&conn);
        let state = UxAgentState {
            last_event_id: Some("4db2f9f8-5100-48af-b335-5c1e7b220f5f".to_string()),
            last_event_at: Some("2026-02-22T12:00:00Z".to_string()),
            last_run_at: Some("2026-02-22T12:05:00Z".to_string()),
        };

        store.set_cursor(&state).unwrap();
        let loaded = store.get_cursor().unwrap();
        assert_eq!(loaded, state);
    }
}
