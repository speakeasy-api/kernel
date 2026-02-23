use std::error::Error;
use std::str::FromStr;

use sqlx::SqlitePool;

use super::learning::{Convention, Correction, CorrectionType};
use super::types::{
    Recommendation, RecommendationAction, RecommendationStatus, RecommendationVersion,
    UxAgentState,
};

pub type StoreResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub struct RecommendationStore {
    pool: SqlitePool,
}

impl RecommendationStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, rec: &Recommendation) -> StoreResult<u64> {
        let action_json = serde_json::to_string(&rec.action)?;
        let result = sqlx::query(
            "INSERT INTO recommendations (trigger_pattern, recommendation, action, status)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&rec.trigger_pattern)
        .bind(&rec.recommendation)
        .bind(&action_json)
        .bind(rec.status.to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid() as u64)
    }

    pub async fn get(&self, id: u64) -> StoreResult<Option<Recommendation>> {
        let row: Option<RecommendationRow> = sqlx::query_as(
            "SELECT id, trigger_pattern, recommendation, action, status
             FROM recommendations WHERE id = ?1",
        )
        .bind(id as i64)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(row_to_recommendation(r)?)),
            None => Ok(None),
        }
    }

    pub async fn list_pending(&self) -> StoreResult<Vec<Recommendation>> {
        self.list_by_status(RecommendationStatus::Pending).await
    }

    pub async fn list_all(&self) -> StoreResult<Vec<Recommendation>> {
        let rows: Vec<RecommendationRow> = sqlx::query_as(
            "SELECT id, trigger_pattern, recommendation, action, status
             FROM recommendations
             ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_recommendation).collect()
    }

    pub async fn update_status(
        &self,
        id: u64,
        status: RecommendationStatus,
    ) -> StoreResult<()> {
        sqlx::query("UPDATE recommendations SET status = ?1 WHERE id = ?2")
            .bind(status.to_string())
            .bind(id as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_cursor(&self) -> StoreResult<UxAgentState> {
        let row: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT last_event_id, last_event_at, last_run_at
             FROM ux_agent_state WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some((last_event_id, last_event_at, last_run_at)) => Ok(UxAgentState {
                last_event_id,
                last_event_at,
                last_run_at,
            }),
            None => Ok(UxAgentState::default()),
        }
    }

    pub async fn set_cursor(&self, state: &UxAgentState) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO ux_agent_state (id, last_event_id, last_event_at, last_run_at)
             VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET
                last_event_id = excluded.last_event_id,
                last_event_at = excluded.last_event_at,
                last_run_at = excluded.last_run_at",
        )
        .bind(&state.last_event_id)
        .bind(&state.last_event_at)
        .bind(&state.last_run_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_version(
        &self,
        recommendation_id: u64,
        version: u32,
        snapshot: &str,
    ) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO recommendation_versions (recommendation_id, version, applied_at, snapshot)
             VALUES (?1, ?2, CURRENT_TIMESTAMP, ?3)",
        )
        .bind(recommendation_id as i64)
        .bind(version)
        .bind(snapshot)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_versions(
        &self,
        recommendation_id: u64,
    ) -> StoreResult<Vec<RecommendationVersion>> {
        let rows: Vec<VersionRow> = sqlx::query_as(
            "SELECT id, recommendation_id, version, applied_at, reverted_at, snapshot
             FROM recommendation_versions
             WHERE recommendation_id = ?1
             ORDER BY version ASC",
        )
        .bind(recommendation_id as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_version).collect())
    }

    pub async fn mark_version_reverted(&self, version_id: u64) -> StoreResult<()> {
        sqlx::query(
            "UPDATE recommendation_versions SET reverted_at = CURRENT_TIMESTAMP WHERE id = ?1",
        )
        .bind(version_id as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_dismissed_patterns(&self) -> StoreResult<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT trigger_pattern FROM recommendations WHERE lower(status) = 'dismissed'",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    // --- Corrections ---

    pub async fn insert_correction(&self, correction: &Correction) -> StoreResult<u64> {
        let result = sqlx::query(
            "INSERT INTO corrections (session_id, correction_type, original_value, corrected_value, context, incorporated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(&correction.session_id)
        .bind(correction.correction_type.as_str())
        .bind(&correction.original_value)
        .bind(&correction.corrected_value)
        .bind(&correction.context)
        .bind(correction.incorporated as i32)
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid() as u64)
    }

    pub async fn get_unincorporated_corrections(&self) -> StoreResult<Vec<Correction>> {
        let rows: Vec<CorrectionRow> = sqlx::query_as(
            "SELECT id, session_id, correction_type, original_value, corrected_value, context, created_at, incorporated
             FROM corrections
             WHERE incorporated = 0
             ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_correction).collect()
    }

    pub async fn get_corrections_by_type(
        &self,
        correction_type: &str,
    ) -> StoreResult<Vec<Correction>> {
        let rows: Vec<CorrectionRow> = sqlx::query_as(
            "SELECT id, session_id, correction_type, original_value, corrected_value, context, created_at, incorporated
             FROM corrections
             WHERE correction_type = ?1
             ORDER BY id ASC",
        )
        .bind(correction_type)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_correction).collect()
    }

    pub async fn mark_corrections_incorporated(&self, ids: &[u64]) -> StoreResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "UPDATE corrections SET incorporated = 1 WHERE id IN ({})",
            placeholders.join(", ")
        );
        let mut query = sqlx::query(&sql);
        for id in ids {
            query = query.bind(*id as i64);
        }
        query.execute(&self.pool).await?;
        Ok(())
    }

    // --- Conventions ---

    pub async fn insert_convention(
        &self,
        convention: &str,
        source_ids: &[u64],
        target_mode: Option<&str>,
    ) -> StoreResult<u64> {
        let source_json = serde_json::to_string(source_ids)?;
        let result = sqlx::query(
            "INSERT INTO conventions (convention, source_corrections, target_mode)
             VALUES (?1, ?2, ?3)",
        )
        .bind(convention)
        .bind(&source_json)
        .bind(target_mode)
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid() as u64)
    }

    pub async fn get_proposed_conventions(&self) -> StoreResult<Vec<Convention>> {
        let rows: Vec<ConventionRow> = sqlx::query_as(
            "SELECT id, convention, source_corrections, target_mode, status, created_at
             FROM conventions
             WHERE status = 'proposed'
             ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_convention).collect()
    }

    pub async fn update_convention_status(&self, id: u64, status: &str) -> StoreResult<()> {
        sqlx::query("UPDATE conventions SET status = ?1 WHERE id = ?2")
            .bind(status)
            .bind(id as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_by_status(
        &self,
        status: RecommendationStatus,
    ) -> StoreResult<Vec<Recommendation>> {
        let rows: Vec<RecommendationRow> = sqlx::query_as(
            "SELECT id, trigger_pattern, recommendation, action, status
             FROM recommendations
             WHERE lower(status) = lower(?1)
             ORDER BY id ASC",
        )
        .bind(status.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_recommendation).collect()
    }
}

// ---- Internal row types ----

#[derive(sqlx::FromRow)]
struct RecommendationRow {
    id: i64,
    trigger_pattern: String,
    recommendation: String,
    action: String,
    status: String,
}

#[derive(sqlx::FromRow)]
struct VersionRow {
    id: i64,
    recommendation_id: i64,
    version: i32,
    applied_at: String,
    reverted_at: Option<String>,
    snapshot: String,
}

#[derive(sqlx::FromRow)]
struct CorrectionRow {
    id: i64,
    session_id: Option<String>,
    correction_type: String,
    original_value: Option<String>,
    corrected_value: Option<String>,
    context: Option<String>,
    created_at: Option<String>,
    incorporated: i32,
}

#[derive(sqlx::FromRow)]
struct ConventionRow {
    id: i64,
    convention: String,
    source_corrections: String,
    target_mode: Option<String>,
    status: Option<String>,
    created_at: Option<String>,
}

fn row_to_recommendation(row: RecommendationRow) -> StoreResult<Recommendation> {
    let action: RecommendationAction = serde_json::from_str(&row.action)?;
    let status = RecommendationStatus::from_str(&row.status)
        .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
    Ok(Recommendation {
        id: row.id as u64,
        trigger_pattern: row.trigger_pattern,
        recommendation: row.recommendation,
        action,
        status,
    })
}

fn row_to_version(row: VersionRow) -> RecommendationVersion {
    RecommendationVersion {
        id: row.id as u64,
        recommendation_id: row.recommendation_id as u64,
        version: row.version as u32,
        applied_at: row.applied_at,
        reverted_at: row.reverted_at,
        snapshot: row.snapshot,
    }
}

fn row_to_correction(row: CorrectionRow) -> StoreResult<Correction> {
    let correction_type = CorrectionType::from_str(&row.correction_type).ok_or_else(|| {
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unknown correction type: {}", row.correction_type),
        ))
    })?;
    Ok(Correction {
        id: row.id as u64,
        session_id: row.session_id,
        correction_type,
        original_value: row.original_value,
        corrected_value: row.corrected_value,
        context: row.context,
        created_at: row.created_at.unwrap_or_default(),
        incorporated: row.incorporated != 0,
    })
}

fn row_to_convention(row: ConventionRow) -> StoreResult<Convention> {
    let source_corrections: Vec<u64> = serde_json::from_str(&row.source_corrections)?;
    Ok(Convention {
        id: row.id as u64,
        convention: row.convention,
        source_corrections,
        target_mode: row.target_mode,
        status: row.status.unwrap_or_default(),
        created_at: row.created_at.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_pool;

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

    #[tokio::test]
    async fn insert_and_get_recommendation() {
        let pool = test_pool().await;
        let store = RecommendationStore::new(pool);
        let rec = sample_recommendation();

        let id = store.insert(&rec).await.unwrap();
        let loaded = store.get(id).await.unwrap().unwrap();

        assert_eq!(loaded.id, id);
        assert_eq!(loaded.trigger_pattern, rec.trigger_pattern);
        assert_eq!(loaded.recommendation, rec.recommendation);
        assert_eq!(loaded.action, rec.action);
        assert_eq!(loaded.status, RecommendationStatus::Pending);
    }

    #[tokio::test]
    async fn list_pending_and_update_status() {
        let pool = test_pool().await;
        let store = RecommendationStore::new(pool);
        let rec = sample_recommendation();
        let id = store.insert(&rec).await.unwrap();

        let pending = store.list_pending().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id);

        store
            .update_status(id, RecommendationStatus::Applied)
            .await
            .unwrap();

        let pending_after = store.list_pending().await.unwrap();
        assert!(pending_after.is_empty());

        let loaded = store.get(id).await.unwrap().unwrap();
        assert_eq!(loaded.status, RecommendationStatus::Applied);
    }

    #[tokio::test]
    async fn list_all_returns_all_recommendations() {
        let pool = test_pool().await;
        let store = RecommendationStore::new(pool);
        let rec = sample_recommendation();
        let first = store.insert(&rec).await.unwrap();
        let second = store.insert(&rec).await.unwrap();

        let all = store.list_all().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, first);
        assert_eq!(all[1].id, second);
    }

    #[tokio::test]
    async fn cursor_defaults_when_empty() {
        let pool = test_pool().await;
        let store = RecommendationStore::new(pool);
        let state = store.get_cursor().await.unwrap();
        assert_eq!(state, UxAgentState::default());
    }

    #[tokio::test]
    async fn cursor_roundtrip() {
        let pool = test_pool().await;
        let store = RecommendationStore::new(pool);
        let state = UxAgentState {
            last_event_id: Some("4db2f9f8-5100-48af-b335-5c1e7b220f5f".to_string()),
            last_event_at: Some("2026-02-22T12:00:00Z".to_string()),
            last_run_at: Some("2026-02-22T12:05:00Z".to_string()),
        };

        store.set_cursor(&state).await.unwrap();
        let loaded = store.get_cursor().await.unwrap();
        assert_eq!(loaded, state);
    }
}
