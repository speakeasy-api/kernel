use std::error::Error;

use serde_json::json;
use sqlx::SqlitePool;
use tracing::{debug, error, info, instrument, warn};

use super::store::RecommendationStore;
use super::types::{ModeChanges, RecommendationAction, RecommendationStatus};

type LifecycleResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// Operations on the mode system (spec 05).
pub trait ModeOperations {
    fn create_mode(
        &self,
        name: &str,
        description: &str,
        system_prompt: &str,
        default_model: Option<&str>,
        allowed_tools: &[String],
    ) -> LifecycleResult<()>;

    fn update_mode(&self, name: &str, changes: &ModeChanges) -> LifecycleResult<()>;

    fn get_mode_snapshot(&self, name: &str) -> LifecycleResult<String>;

    fn restore_mode_snapshot(&self, name: &str, snapshot: &str) -> LifecycleResult<()>;
}

/// Operations on the config system (spec 03).
pub trait ConfigOperations {
    fn set_config(&self, key: &str, value: &str) -> LifecycleResult<()>;
    fn get_config(&self, key: &str) -> LifecycleResult<String>;
    fn restore_config(&self, key: &str, value: &str) -> LifecycleResult<()>;
}

pub struct StubModeOps;

impl ModeOperations for StubModeOps {
    fn create_mode(
        &self,
        _name: &str,
        _description: &str,
        _system_prompt: &str,
        _default_model: Option<&str>,
        _allowed_tools: &[String],
    ) -> LifecycleResult<()> {
        Ok(())
    }

    fn update_mode(&self, _name: &str, _changes: &ModeChanges) -> LifecycleResult<()> {
        Ok(())
    }

    fn get_mode_snapshot(&self, _name: &str) -> LifecycleResult<String> {
        Ok("{}".to_string())
    }

    fn restore_mode_snapshot(&self, _name: &str, _snapshot: &str) -> LifecycleResult<()> {
        Ok(())
    }
}

pub struct StubConfigOps;

impl ConfigOperations for StubConfigOps {
    fn set_config(&self, _key: &str, _value: &str) -> LifecycleResult<()> {
        Ok(())
    }

    fn get_config(&self, _key: &str) -> LifecycleResult<String> {
        Ok(String::new())
    }

    fn restore_config(&self, _key: &str, _value: &str) -> LifecycleResult<()> {
        Ok(())
    }
}

pub struct LifecycleManager;

impl LifecycleManager {
    /// Apply a pending recommendation.
    ///
    /// 1. Validate recommendation is in Pending status
    /// 2. Capture current state as snapshot (for rollback)
    /// 3. Execute the action side-effect
    /// 4. Save version record with snapshot
    /// 5. Update recommendation status to Applied
    #[instrument(skip(pool, mode_ops, config_ops))]
    pub async fn apply(
        pool: &SqlitePool,
        recommendation_id: u64,
        mode_ops: &dyn ModeOperations,
        config_ops: &dyn ConfigOperations,
    ) -> LifecycleResult<()> {
        info!(recommendation_id, "applying recommendation");
        let store = RecommendationStore::new(pool.clone());

        let rec = store
            .get(recommendation_id)
            .await?
            .ok_or_else(|| {
                error!(recommendation_id, "recommendation not found");
                lifecycle_error(&format!("recommendation {recommendation_id} not found"))
            })?;

        if rec.status != RecommendationStatus::Pending {
            error!(
                recommendation_id,
                status = %rec.status,
                "cannot apply recommendation in non-Pending status"
            );
            return Err(lifecycle_error(&format!(
                "cannot apply recommendation in {} status, expected Pending",
                rec.status
            )));
        }

        let snapshot = capture_snapshot(&rec.action, mode_ops, config_ops)?;
        execute_action(&rec.action, mode_ops, config_ops)?;

        let versions = store.get_versions(recommendation_id).await?;
        let next_version = versions.last().map_or(1, |v| v.version + 1);

        store.insert_version(recommendation_id, next_version, &snapshot).await?;
        store.update_status(recommendation_id, RecommendationStatus::Applied).await?;

        info!(
            recommendation_id,
            version = next_version,
            "recommendation applied successfully"
        );
        Ok(())
    }

    /// Dismiss a pending recommendation.
    ///
    /// 1. Validate recommendation is in Pending status
    /// 2. Update status to Dismissed
    #[instrument(skip(pool))]
    pub async fn dismiss(pool: &SqlitePool, recommendation_id: u64) -> LifecycleResult<()> {
        info!(recommendation_id, "dismissing recommendation");
        let store = RecommendationStore::new(pool.clone());

        let rec = store
            .get(recommendation_id)
            .await?
            .ok_or_else(|| {
                error!(recommendation_id, "recommendation not found");
                lifecycle_error(&format!("recommendation {recommendation_id} not found"))
            })?;

        if rec.status != RecommendationStatus::Pending {
            error!(
                recommendation_id,
                status = %rec.status,
                "cannot dismiss recommendation in non-Pending status"
            );
            return Err(lifecycle_error(&format!(
                "cannot dismiss recommendation in {} status, expected Pending",
                rec.status
            )));
        }

        store.update_status(recommendation_id, RecommendationStatus::Dismissed).await?;

        info!(recommendation_id, "recommendation dismissed");
        Ok(())
    }

    /// Revert an applied recommendation.
    ///
    /// 1. Validate recommendation is in Applied status
    /// 2. Load the most recent version record
    /// 3. Apply the snapshot (restore previous state)
    /// 4. Mark version as reverted
    /// 5. Update recommendation status to Reverted
    #[instrument(skip(pool, mode_ops, config_ops))]
    pub async fn revert(
        pool: &SqlitePool,
        recommendation_id: u64,
        mode_ops: &dyn ModeOperations,
        config_ops: &dyn ConfigOperations,
    ) -> LifecycleResult<()> {
        info!(recommendation_id, "reverting recommendation");
        let store = RecommendationStore::new(pool.clone());

        let rec = store
            .get(recommendation_id)
            .await?
            .ok_or_else(|| {
                error!(recommendation_id, "recommendation not found");
                lifecycle_error(&format!("recommendation {recommendation_id} not found"))
            })?;

        if rec.status != RecommendationStatus::Applied {
            error!(
                recommendation_id,
                status = %rec.status,
                "cannot revert recommendation in non-Applied status"
            );
            return Err(lifecycle_error(&format!(
                "cannot revert recommendation in {} status, expected Applied",
                rec.status
            )));
        }

        let versions = store.get_versions(recommendation_id).await?;
        let latest = versions
            .last()
            .ok_or_else(|| {
                error!(recommendation_id, "no version records found for applied recommendation");
                lifecycle_error("no version records found for applied recommendation")
            })?;

        restore_snapshot(&rec.action, &latest.snapshot, mode_ops, config_ops)?;
        store.mark_version_reverted(latest.id).await?;
        store.update_status(recommendation_id, RecommendationStatus::Reverted).await?;

        info!(
            recommendation_id,
            version_id = latest.id,
            "recommendation reverted successfully"
        );
        Ok(())
    }
}

#[instrument(skip(mode_ops, _config_ops))]
fn capture_snapshot(
    action: &RecommendationAction,
    mode_ops: &dyn ModeOperations,
    _config_ops: &dyn ConfigOperations,
) -> LifecycleResult<String> {
    debug!(?action, "capturing snapshot before action");
    let snapshot = match action {
        RecommendationAction::ModeCreate { .. } => {
            json!({"existed": false}).to_string()
        }
        RecommendationAction::ModeEdit { mode_name, .. } => {
            mode_ops.get_mode_snapshot(mode_name)?
        }
        RecommendationAction::ModelChange { role, from_model, .. } => {
            json!({"role": role, "model": from_model}).to_string()
        }
        RecommendationAction::ConfigChange { key, old_value, .. } => {
            json!({"key": key, "value": old_value}).to_string()
        }
        RecommendationAction::PromptEdit {
            mode_name,
            old_fragment,
            ..
        } => {
            json!({"mode_name": mode_name, "fragment": old_fragment}).to_string()
        }
    };
    Ok(snapshot)
}

#[instrument(skip(mode_ops, config_ops))]
fn execute_action(
    action: &RecommendationAction,
    mode_ops: &dyn ModeOperations,
    config_ops: &dyn ConfigOperations,
) -> LifecycleResult<()> {
    info!(?action, "executing recommendation action");
    match action {
        RecommendationAction::ModeCreate {
            name,
            description,
            system_prompt,
            default_model,
            allowed_tools,
        } => {
            mode_ops.create_mode(name, description, system_prompt, default_model.as_deref(), allowed_tools)?;
        }
        RecommendationAction::ModeEdit { mode_name, changes } => {
            mode_ops.update_mode(mode_name, changes)?;
        }
        RecommendationAction::ModelChange { role, to_model, .. } => {
            config_ops.set_config(&format!("model.{role}"), to_model)?;
        }
        RecommendationAction::ConfigChange { key, new_value, .. } => {
            config_ops.set_config(key, new_value)?;
        }
        RecommendationAction::PromptEdit {
            mode_name,
            new_fragment,
            ..
        } => {
            let changes = ModeChanges {
                system_prompt: Some(new_fragment.clone()),
                description: None,
                default_model: None,
                allowed_tools: None,
            };
            mode_ops.update_mode(mode_name, &changes)?;
        }
    }
    Ok(())
}

#[instrument(skip(snapshot, mode_ops, config_ops))]
fn restore_snapshot(
    action: &RecommendationAction,
    snapshot: &str,
    mode_ops: &dyn ModeOperations,
    config_ops: &dyn ConfigOperations,
) -> LifecycleResult<()> {
    info!(?action, "restoring snapshot");
    match action {
        RecommendationAction::ModeCreate { name, .. } => {
            mode_ops.restore_mode_snapshot(name, snapshot)?;
        }
        RecommendationAction::ModeEdit { mode_name, .. } => {
            mode_ops.restore_mode_snapshot(mode_name, snapshot)?;
        }
        RecommendationAction::ModelChange { role, .. } => {
            let snap: serde_json::Value = serde_json::from_str(snapshot)?;
            let model = snap["model"]
                .as_str()
                .ok_or_else(|| lifecycle_error("missing model in snapshot"))?;
            config_ops.restore_config(&format!("model.{role}"), model)?;
        }
        RecommendationAction::ConfigChange { .. } => {
            let snap: serde_json::Value = serde_json::from_str(snapshot)?;
            let key = snap["key"]
                .as_str()
                .ok_or_else(|| lifecycle_error("missing key in snapshot"))?;
            let value = snap["value"]
                .as_str()
                .ok_or_else(|| lifecycle_error("missing value in snapshot"))?;
            config_ops.restore_config(key, value)?;
        }
        RecommendationAction::PromptEdit { mode_name, .. } => {
            let snap: serde_json::Value = serde_json::from_str(snapshot)?;
            let fragment = snap["fragment"]
                .as_str()
                .ok_or_else(|| lifecycle_error("missing fragment in snapshot"))?;
            let changes = ModeChanges {
                system_prompt: Some(fragment.to_string()),
                description: None,
                default_model: None,
                allowed_tools: None,
            };
            mode_ops.update_mode(mode_name, &changes)?;
        }
    }
    Ok(())
}

fn lifecycle_error(msg: &str) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        msg.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_pool;
    use crate::ux_agent::types::{Recommendation, RecommendationAction, RecommendationStatus};

    async fn insert_pending(pool: &SqlitePool, action: RecommendationAction) -> u64 {
        let store = RecommendationStore::new(pool.clone());
        store
            .insert(&Recommendation {
                id: 0,
                trigger_pattern: "test-pattern".to_string(),
                recommendation: "test recommendation".to_string(),
                action,
                status: RecommendationStatus::Pending,
            })
            .await
            .unwrap()
    }

    fn model_change_action() -> RecommendationAction {
        RecommendationAction::ModelChange {
            role: "planner".to_string(),
            from_model: "cheap".to_string(),
            to_model: "smart".to_string(),
        }
    }

    #[tokio::test]
    async fn apply_transitions_pending_to_applied() {
        let pool = test_pool().await;
        let id = insert_pending(&pool, model_change_action()).await;

        LifecycleManager::apply(&pool, id, &StubModeOps, &StubConfigOps).await.unwrap();

        let store = RecommendationStore::new(pool.clone());
        let rec = store.get(id).await.unwrap().unwrap();
        assert_eq!(rec.status, RecommendationStatus::Applied);
    }

    #[tokio::test]
    async fn apply_creates_version_record() {
        let pool = test_pool().await;
        let id = insert_pending(&pool, model_change_action()).await;

        LifecycleManager::apply(&pool, id, &StubModeOps, &StubConfigOps).await.unwrap();

        let store = RecommendationStore::new(pool.clone());
        let versions = store.get_versions(id).await.unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, 1);
        assert_eq!(versions[0].recommendation_id, id);
        assert!(versions[0].reverted_at.is_none());

        let snap: serde_json::Value = serde_json::from_str(&versions[0].snapshot).unwrap();
        assert_eq!(snap["role"], "planner");
        assert_eq!(snap["model"], "cheap");
    }

    #[tokio::test]
    async fn apply_rejects_non_pending() {
        let pool = test_pool().await;
        let id = insert_pending(&pool, model_change_action()).await;

        LifecycleManager::apply(&pool, id, &StubModeOps, &StubConfigOps).await.unwrap();

        let err = LifecycleManager::apply(&pool, id, &StubModeOps, &StubConfigOps).await.unwrap_err();
        assert!(err.to_string().contains("expected Pending"));
    }

    #[tokio::test]
    async fn dismiss_transitions_pending_to_dismissed() {
        let pool = test_pool().await;
        let id = insert_pending(&pool, model_change_action()).await;

        LifecycleManager::dismiss(&pool, id).await.unwrap();

        let store = RecommendationStore::new(pool.clone());
        let rec = store.get(id).await.unwrap().unwrap();
        assert_eq!(rec.status, RecommendationStatus::Dismissed);
    }

    #[tokio::test]
    async fn dismiss_rejects_non_pending() {
        let pool = test_pool().await;
        let id = insert_pending(&pool, model_change_action()).await;

        LifecycleManager::apply(&pool, id, &StubModeOps, &StubConfigOps).await.unwrap();

        let err = LifecycleManager::dismiss(&pool, id).await.unwrap_err();
        assert!(err.to_string().contains("expected Pending"));
    }

    #[tokio::test]
    async fn revert_transitions_applied_to_reverted() {
        let pool = test_pool().await;
        let id = insert_pending(&pool, model_change_action()).await;

        LifecycleManager::apply(&pool, id, &StubModeOps, &StubConfigOps).await.unwrap();
        LifecycleManager::revert(&pool, id, &StubModeOps, &StubConfigOps).await.unwrap();

        let store = RecommendationStore::new(pool.clone());
        let rec = store.get(id).await.unwrap().unwrap();
        assert_eq!(rec.status, RecommendationStatus::Reverted);

        let versions = store.get_versions(id).await.unwrap();
        assert_eq!(versions.len(), 1);
        assert!(versions[0].reverted_at.is_some());
    }

    #[tokio::test]
    async fn revert_rejects_non_applied() {
        let pool = test_pool().await;
        let id = insert_pending(&pool, model_change_action()).await;

        let err = LifecycleManager::revert(&pool, id, &StubModeOps, &StubConfigOps).await.unwrap_err();
        assert!(err.to_string().contains("expected Applied"));
    }

    #[tokio::test]
    async fn revert_rejects_dismissed() {
        let pool = test_pool().await;
        let id = insert_pending(&pool, model_change_action()).await;

        LifecycleManager::dismiss(&pool, id).await.unwrap();

        let err = LifecycleManager::revert(&pool, id, &StubModeOps, &StubConfigOps).await.unwrap_err();
        assert!(err.to_string().contains("expected Applied"));
    }

    #[tokio::test]
    async fn apply_not_found() {
        let pool = test_pool().await;
        let err = LifecycleManager::apply(&pool, 999, &StubModeOps, &StubConfigOps).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn get_dismissed_patterns_returns_trigger_patterns() {
        let pool = test_pool().await;
        let id1 = insert_pending(&pool, model_change_action()).await;
        let id2 = insert_pending(
            &pool,
            RecommendationAction::ConfigChange {
                key: "timeout".to_string(),
                old_value: "30".to_string(),
                new_value: "60".to_string(),
            },
        )
        .await;
        // One applied, one dismissed
        LifecycleManager::apply(&pool, id1, &StubModeOps, &StubConfigOps).await.unwrap();
        LifecycleManager::dismiss(&pool, id2).await.unwrap();

        let store = RecommendationStore::new(pool.clone());
        let patterns = store.get_dismissed_patterns().await.unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0], "test-pattern");
    }

    #[tokio::test]
    async fn apply_mode_create_snapshot() {
        let pool = test_pool().await;
        let id = insert_pending(
            &pool,
            RecommendationAction::ModeCreate {
                name: "db-mode".to_string(),
                description: "Database mode".to_string(),
                system_prompt: "You are a DB expert".to_string(),
                default_model: None,
                allowed_tools: vec!["sql".to_string()],
            },
        )
        .await;

        LifecycleManager::apply(&pool, id, &StubModeOps, &StubConfigOps).await.unwrap();

        let store = RecommendationStore::new(pool.clone());
        let versions = store.get_versions(id).await.unwrap();
        let snap: serde_json::Value = serde_json::from_str(&versions[0].snapshot).unwrap();
        assert_eq!(snap["existed"], false);
    }

    #[tokio::test]
    async fn apply_config_change_snapshot() {
        let pool = test_pool().await;
        let id = insert_pending(
            &pool,
            RecommendationAction::ConfigChange {
                key: "timeout".to_string(),
                old_value: "30".to_string(),
                new_value: "60".to_string(),
            },
        )
        .await;

        LifecycleManager::apply(&pool, id, &StubModeOps, &StubConfigOps).await.unwrap();

        let store = RecommendationStore::new(pool.clone());
        let versions = store.get_versions(id).await.unwrap();
        let snap: serde_json::Value = serde_json::from_str(&versions[0].snapshot).unwrap();
        assert_eq!(snap["key"], "timeout");
        assert_eq!(snap["value"], "30");
    }

    #[tokio::test]
    async fn apply_prompt_edit_snapshot() {
        let pool = test_pool().await;
        let id = insert_pending(
            &pool,
            RecommendationAction::PromptEdit {
                mode_name: "coding".to_string(),
                old_fragment: "be concise".to_string(),
                new_fragment: "be verbose".to_string(),
            },
        )
        .await;

        LifecycleManager::apply(&pool, id, &StubModeOps, &StubConfigOps).await.unwrap();

        let store = RecommendationStore::new(pool.clone());
        let versions = store.get_versions(id).await.unwrap();
        let snap: serde_json::Value = serde_json::from_str(&versions[0].snapshot).unwrap();
        assert_eq!(snap["mode_name"], "coding");
        assert_eq!(snap["fragment"], "be concise");
    }
}
