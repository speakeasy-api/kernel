use serde::Deserialize;
use sqlx::SqlitePool;
use tracing::{debug, error, info, instrument};

use super::prompt::{self, UX_AGENT_SYSTEM_PROMPT};
use super::store::RecommendationStore;
use super::triggers::{EventSummary, TriggerReason};
use super::types::{Recommendation, RecommendationAction, RecommendationStatus, UxAgentState};

type RuntimeError = Box<dyn std::error::Error + Send + Sync>;

pub trait ModelInvoker: Send + Sync {
    fn invoke(&self, system_prompt: &str, user_message: &str) -> Result<String, RuntimeError>;
}

pub struct StubModelInvoker;

impl ModelInvoker for StubModelInvoker {
    fn invoke(&self, _system_prompt: &str, _user_message: &str) -> Result<String, RuntimeError> {
        Ok(serde_json::to_string(&serde_json::json!({
            "recommendations": []
        }))?)
    }
}

pub struct UxAgentRuntime {
    model: String,
    invoker: Box<dyn ModelInvoker>,
}

#[derive(Debug, Deserialize)]
struct ModelResponse {
    recommendations: Vec<RawRecommendation>,
}

#[derive(Debug, Deserialize)]
struct RawRecommendation {
    trigger_pattern: String,
    recommendation: String,
    action: RecommendationAction,
}

impl UxAgentRuntime {
    #[instrument(skip(invoker))]
    pub fn new(model: String, invoker: Box<dyn ModelInvoker>) -> Self {
        info!(model = %model, "UX agent runtime created");
        Self { model, invoker }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Main entry point. Called by the event accumulator when triggers fire.
    ///
    /// 1. Load cursor state from DB
    /// 2. Assemble context (triggers, aggregated data, config, modes, dismissed patterns)
    /// 3. Build the prompt
    /// 4. Invoke the model
    /// 5. Parse model response into Recommendations
    /// 6. Persist recommendations with status=Pending
    /// 7. Update cursor state
    /// 8. Return the new recommendations
    #[instrument(skip(self, pool, summary, config_snapshot, modes_snapshot), fields(model = %self.model, trigger_count = triggers.len()))]
    pub async fn run(
        &self,
        pool: &SqlitePool,
        triggers: &[TriggerReason],
        summary: &EventSummary,
        config_snapshot: &str,
        modes_snapshot: &str,
    ) -> Result<Vec<Recommendation>, RuntimeError> {
        info!(
            model = %self.model,
            trigger_count = triggers.len(),
            "UX agent run starting"
        );
        let store = RecommendationStore::new(pool.clone());

        // 1. Load cursor state
        debug!("loading cursor state");
        let _cursor = store.get_cursor().await?;

        // 2-3. Assemble context and build prompt
        debug!("assembling context and building prompt");
        let dismissed = store.get_dismissed_patterns().await?;
        let user_message = prompt::build_user_message(
            triggers,
            summary,
            config_snapshot,
            modes_snapshot,
            &dismissed,
        );

        // 4. Invoke model
        debug!(model = %self.model, "invoking model");
        let response_text = self
            .invoker
            .invoke(UX_AGENT_SYSTEM_PROMPT, &user_message)
            .map_err(|e| {
                error!(model = %self.model, error = %e, "model invocation failed");
                e
            })?;

        // 5. Parse response
        debug!("parsing model response");
        let parsed: ModelResponse = serde_json::from_str(&response_text).map_err(|e| {
            error!(error = %e, "failed to parse model response as JSON");
            e
        })?;

        // 6. Persist recommendations
        debug!(
            count = parsed.recommendations.len(),
            "persisting recommendations"
        );
        let mut recommendations = Vec::with_capacity(parsed.recommendations.len());
        for raw in parsed.recommendations {
            let rec = Recommendation {
                id: 0,
                trigger_pattern: raw.trigger_pattern,
                recommendation: raw.recommendation,
                action: raw.action,
                status: RecommendationStatus::Pending,
            };
            let id = store.insert(&rec).await?;
            debug!(recommendation_id = id, trigger_pattern = %rec.trigger_pattern, "recommendation persisted");
            recommendations.push(Recommendation { id, ..rec });
        }

        // 7. Update cursor state
        debug!("updating cursor state");
        let now = chrono::Utc::now().to_rfc3339();
        let new_cursor = UxAgentState {
            last_event_id: summary
                .tool_failures
                .last()
                .map(|(name, _)| name.clone())
                .or(_cursor.last_event_id),
            last_event_at: Some(now.clone()),
            last_run_at: Some(now),
        };
        store.set_cursor(&new_cursor).await?;

        // 8. Return
        info!(
            recommendation_count = recommendations.len(),
            "UX agent run completed"
        );
        Ok(recommendations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_pool;
    use crate::ux_agent::store::RecommendationStore;
    use crate::ux_agent::triggers::TriggerReason;
    use crate::ux_agent::types::RecommendationStatus;

    struct EchoInvoker {
        response: String,
    }

    impl ModelInvoker for EchoInvoker {
        fn invoke(
            &self,
            _system_prompt: &str,
            _user_message: &str,
        ) -> Result<String, RuntimeError> {
            Ok(self.response.clone())
        }
    }

    struct FailingInvoker;

    impl ModelInvoker for FailingInvoker {
        fn invoke(
            &self,
            _system_prompt: &str,
            _user_message: &str,
        ) -> Result<String, RuntimeError> {
            Err("model unavailable".into())
        }
    }

    fn default_summary() -> EventSummary {
        EventSummary {
            rejection_count: 3,
            has_new_events: true,
            ..Default::default()
        }
    }

    #[test]
    fn new_creates_runtime_with_model() {
        let rt = UxAgentRuntime::new("cheap-model".into(), Box::new(StubModelInvoker));
        assert_eq!(rt.model(), "cheap-model");
    }

    #[tokio::test]
    async fn run_with_stub_returns_empty_recommendations() {
        let pool = test_pool().await;
        let rt = UxAgentRuntime::new("test-model".into(), Box::new(StubModelInvoker));
        let triggers = vec![TriggerReason::RejectionsAccumulated { count: 3 }];

        let recs = rt
            .run(&pool, &triggers, &default_summary(), "{}", "[]")
            .await
            .unwrap();
        assert!(recs.is_empty());
    }

    #[tokio::test]
    async fn run_persists_recommendations_and_updates_cursor() {
        let pool = test_pool().await;
        let response = serde_json::json!({
            "recommendations": [
                {
                    "trigger_pattern": "3+ rejections on schema edits",
                    "recommendation": "Switch planner model",
                    "action": {
                        "type": "ModelChange",
                        "role": "planner",
                        "from_model": "cheap",
                        "to_model": "smart"
                    }
                }
            ]
        });
        let invoker = EchoInvoker {
            response: serde_json::to_string(&response).unwrap(),
        };
        let rt = UxAgentRuntime::new("test-model".into(), Box::new(invoker));
        let triggers = vec![TriggerReason::RejectionsAccumulated { count: 3 }];

        let recs = rt
            .run(&pool, &triggers, &default_summary(), "{}", "[]")
            .await
            .unwrap();

        assert_eq!(recs.len(), 1);
        assert_ne!(recs[0].id, 0);
        assert_eq!(recs[0].status, RecommendationStatus::Pending);
        assert_eq!(recs[0].trigger_pattern, "3+ rejections on schema edits");

        // Verify persisted
        let store = RecommendationStore::new(pool.clone());
        let pending = store.list_pending().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, recs[0].id);

        // Verify cursor updated
        let cursor = store.get_cursor().await.unwrap();
        assert!(cursor.last_run_at.is_some());
    }

    #[tokio::test]
    async fn run_returns_error_on_model_failure() {
        let pool = test_pool().await;
        let rt = UxAgentRuntime::new("test-model".into(), Box::new(FailingInvoker));
        let triggers = vec![TriggerReason::NewSession];

        let result = rt
            .run(&pool, &triggers, &default_summary(), "{}", "[]")
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("model unavailable"));
    }

    #[tokio::test]
    async fn run_returns_error_on_invalid_json() {
        let invoker = EchoInvoker {
            response: "not valid json".to_string(),
        };
        let pool = test_pool().await;
        let rt = UxAgentRuntime::new("test-model".into(), Box::new(invoker));
        let triggers = vec![TriggerReason::NewSession];

        let result = rt
            .run(&pool, &triggers, &default_summary(), "{}", "[]")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_with_multiple_recommendations() {
        let pool = test_pool().await;
        let response = serde_json::json!({
            "recommendations": [
                {
                    "trigger_pattern": "cost spike",
                    "recommendation": "Switch to cheaper model",
                    "action": {
                        "type": "ModelChange",
                        "role": "coder",
                        "from_model": "expensive",
                        "to_model": "cheap"
                    }
                },
                {
                    "trigger_pattern": "repeated schema work",
                    "recommendation": "Create a DB mode",
                    "action": {
                        "type": "ModeCreate",
                        "name": "db-mode",
                        "description": "Database schema work",
                        "system_prompt": "You are a DB specialist",
                        "default_model": null,
                        "allowed_tools": ["sql", "migrate"]
                    }
                }
            ]
        });
        let invoker = EchoInvoker {
            response: serde_json::to_string(&response).unwrap(),
        };
        let rt = UxAgentRuntime::new("test-model".into(), Box::new(invoker));
        let triggers = vec![TriggerReason::RejectionsAccumulated { count: 5 }];

        let recs = rt
            .run(&pool, &triggers, &default_summary(), "{}", "[]")
            .await
            .unwrap();
        assert_eq!(recs.len(), 2);

        let store = RecommendationStore::new(pool.clone());
        let all = store.list_all().await.unwrap();
        assert_eq!(all.len(), 2);
    }
}
