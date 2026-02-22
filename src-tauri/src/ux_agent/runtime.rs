use rusqlite::Connection;
use serde::Deserialize;

use super::prompt::{self, UX_AGENT_SYSTEM_PROMPT};
use super::store::RecommendationStore;
use super::triggers::{EventSummary, TriggerReason};
use super::types::{Recommendation, RecommendationAction, RecommendationStatus, UxAgentState};

type RuntimeError = Box<dyn std::error::Error + Send + Sync>;

pub trait ModelInvoker: Send + Sync {
    fn invoke(
        &self,
        system_prompt: &str,
        user_message: &str,
    ) -> Result<String, RuntimeError>;
}

pub struct StubModelInvoker;

impl ModelInvoker for StubModelInvoker {
    fn invoke(
        &self,
        _system_prompt: &str,
        _user_message: &str,
    ) -> Result<String, RuntimeError> {
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
    pub fn new(model: String, invoker: Box<dyn ModelInvoker>) -> Self {
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
    pub fn run(
        &self,
        conn: &Connection,
        triggers: &[TriggerReason],
        summary: &EventSummary,
        config_snapshot: &str,
        modes_snapshot: &str,
    ) -> Result<Vec<Recommendation>, RuntimeError> {
        let store = RecommendationStore::new(conn);

        // 1. Load cursor state
        let _cursor = store.get_cursor()?;

        // 2-3. Assemble context and build prompt
        let dismissed = store.get_dismissed_patterns()?;
        let user_message = prompt::build_user_message(
            triggers,
            summary,
            config_snapshot,
            modes_snapshot,
            &dismissed,
        );

        // 4. Invoke model
        let response_text = self.invoker.invoke(UX_AGENT_SYSTEM_PROMPT, &user_message)?;

        // 5. Parse response
        let parsed: ModelResponse = serde_json::from_str(&response_text)?;

        // 6. Persist recommendations
        let mut recommendations = Vec::with_capacity(parsed.recommendations.len());
        for raw in parsed.recommendations {
            let rec = Recommendation {
                id: 0,
                trigger_pattern: raw.trigger_pattern,
                recommendation: raw.recommendation,
                action: raw.action,
                status: RecommendationStatus::Pending,
            };
            let id = store.insert(&rec)?;
            recommendations.push(Recommendation { id, ..rec });
        }

        // 7. Update cursor state
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
        store.set_cursor(&new_cursor)?;

        // 8. Return
        Ok(recommendations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        RecommendationStore::ensure_tables(&conn).unwrap();
        conn
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

    #[test]
    fn run_with_stub_returns_empty_recommendations() {
        let conn = setup();
        let rt = UxAgentRuntime::new("test-model".into(), Box::new(StubModelInvoker));
        let triggers = vec![TriggerReason::RejectionsAccumulated { count: 3 }];

        let recs = rt.run(&conn, &triggers, &default_summary(), "{}", "[]").unwrap();
        assert!(recs.is_empty());
    }

    #[test]
    fn run_persists_recommendations_and_updates_cursor() {
        let conn = setup();
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

        let recs = rt.run(&conn, &triggers, &default_summary(), "{}", "[]").unwrap();

        assert_eq!(recs.len(), 1);
        assert_ne!(recs[0].id, 0);
        assert_eq!(recs[0].status, RecommendationStatus::Pending);
        assert_eq!(recs[0].trigger_pattern, "3+ rejections on schema edits");

        // Verify persisted
        let store = RecommendationStore::new(&conn);
        let pending = store.list_pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, recs[0].id);

        // Verify cursor updated
        let cursor = store.get_cursor().unwrap();
        assert!(cursor.last_run_at.is_some());
    }

    #[test]
    fn run_returns_error_on_model_failure() {
        let conn = setup();
        let rt = UxAgentRuntime::new("test-model".into(), Box::new(FailingInvoker));
        let triggers = vec![TriggerReason::NewSession];

        let result = rt.run(&conn, &triggers, &default_summary(), "{}", "[]");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("model unavailable"));
    }

    #[test]
    fn run_returns_error_on_invalid_json() {
        let invoker = EchoInvoker {
            response: "not valid json".to_string(),
        };
        let conn = setup();
        let rt = UxAgentRuntime::new("test-model".into(), Box::new(invoker));
        let triggers = vec![TriggerReason::NewSession];

        let result = rt.run(&conn, &triggers, &default_summary(), "{}", "[]");
        assert!(result.is_err());
    }

    #[test]
    fn run_with_multiple_recommendations() {
        let conn = setup();
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

        let recs = rt.run(&conn, &triggers, &default_summary(), "{}", "[]").unwrap();
        assert_eq!(recs.len(), 2);

        let store = RecommendationStore::new(&conn);
        let all = store.list_all().unwrap();
        assert_eq!(all.len(), 2);
    }
}
