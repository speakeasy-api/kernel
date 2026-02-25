use chrono::NaiveDateTime;
use sqlx::SqlitePool;
use tracing::{debug, error};
use uuid::Uuid;

use crate::db::queries;
use crate::events::{Event, EventData, EventMetadata};

#[derive(Debug)]
pub enum EmitError {
    Db(sqlx::Error),
    Json(serde_json::Error),
    InvalidUuid(uuid::Error),
    InvalidTimestamp(String),
}

impl std::fmt::Display for EmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "database error: {e}"),
            Self::Json(e) => write!(f, "json error: {e}"),
            Self::InvalidUuid(e) => write!(f, "invalid uuid: {e}"),
            Self::InvalidTimestamp(s) => write!(f, "invalid timestamp: {s}"),
        }
    }
}

impl std::error::Error for EmitError {}

impl From<sqlx::Error> for EmitError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

impl From<serde_json::Error> for EmitError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl From<uuid::Error> for EmitError {
    fn from(e: uuid::Error) -> Self {
        Self::InvalidUuid(e)
    }
}

/// Write a typed event to the SQLite events table.
///
/// Generates a UUID v4 for the event, derives `kind` from the `EventData`
/// variant name, serializes variant fields as JSON for the `data` column,
/// and returns a fully-populated `Event` with a DB-authoritative timestamp.
pub async fn emit(
    pool: &SqlitePool,
    session_id: &str,
    agent_id: Option<&str>,
    data: EventData,
) -> Result<Event, EmitError> {
    let kind = data.kind();
    debug!(kind = %kind, session_id = %session_id, agent_id = ?agent_id, "emitting event");

    // Serialize EventData, extract only the variant fields (the "data" content).
    // EventData uses #[serde(tag = "kind", content = "data")], so the serialized
    // form is {"kind":"...", "data":{...}}. We store only the inner fields.
    let json_value = serde_json::to_value(&data)?;
    let data_json = match json_value.get("data") {
        Some(v) => serde_json::to_string(v)?,
        None => "{}".to_string(),
    };

    // Insert into DB (generates UUID v4 for event id)
    let db_event = queries::insert_event(pool, session_id, agent_id, kind, &data_json).await?;

    // Parse DB-authoritative timestamp (SQLite CURRENT_TIMESTAMP format)
    let timestamp = NaiveDateTime::parse_from_str(&db_event.created_at, "%Y-%m-%d %H:%M:%S")
        .map(|ndt| ndt.and_utc())
        .map_err(|_| {
            error!(kind = %kind, session_id = %session_id, timestamp = %db_event.created_at, "invalid timestamp from DB");
            EmitError::InvalidTimestamp(db_event.created_at.clone())
        })?;

    Ok(Event {
        metadata: EventMetadata {
            id: Uuid::parse_str(&db_event.id)?,
            timestamp,
            session_id: Uuid::parse_str(&db_event.session_id)?,
            agent_id: db_event
                .agent_id
                .as_deref()
                .map(Uuid::parse_str)
                .transpose()?,
        },
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{queries::create_session, queries::events_since, test_pool};
    use crate::events::{AgentRole, DiffStat, TokenMetrics};

    #[tokio::test]
    async fn emit_writes_event_to_db() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        let data = EventData::PromptSubmitted {
            prompt: "hello world".into(),
        };
        let event = emit(&pool, &session.id, None, data).await.unwrap();

        // Verify event exists in DB
        let db_events = events_since(&pool, &session.id, "2000-01-01").await.unwrap();
        assert_eq!(db_events.len(), 1);
        assert_eq!(db_events[0].id, event.metadata.id.to_string());
    }

    #[tokio::test]
    async fn emit_returns_correct_kind() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        let data = EventData::PromptSubmitted {
            prompt: "test".into(),
        };
        let event = emit(&pool, &session.id, None, data).await.unwrap();
        assert_eq!(event.data.kind(), "PromptSubmitted");

        // Verify kind column in DB matches
        let db_events = events_since(&pool, &session.id, "2000-01-01").await.unwrap();
        assert_eq!(db_events[0].kind, "PromptSubmitted");
    }

    #[tokio::test]
    async fn emit_stores_variant_fields_only_in_data_column() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        let data = EventData::PromptSubmitted {
            prompt: "hello".into(),
        };
        emit(&pool, &session.id, None, data).await.unwrap();

        let db_events = events_since(&pool, &session.id, "2000-01-01").await.unwrap();
        let stored: serde_json::Value = serde_json::from_str(&db_events[0].data).unwrap();

        // Should contain only the variant fields, not the kind tag
        assert_eq!(stored["prompt"], "hello");
        assert!(stored.get("kind").is_none());
    }

    #[tokio::test]
    async fn emit_generates_uuid_v4() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        let data = EventData::PromptSubmitted {
            prompt: "test".into(),
        };
        let event = emit(&pool, &session.id, None, data).await.unwrap();

        // UUID should be valid v4
        assert_eq!(event.metadata.id.get_version_num(), 4);
    }

    #[tokio::test]
    async fn emit_returns_db_authoritative_timestamp() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        let data = EventData::PromptSubmitted {
            prompt: "test".into(),
        };
        let event = emit(&pool, &session.id, None, data).await.unwrap();

        // Timestamp should be populated (not epoch)
        assert!(event.metadata.timestamp.timestamp() > 0);
    }

    #[tokio::test]
    async fn emit_with_agent_id() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();
        let agent_id = Uuid::new_v4().to_string();

        let data = EventData::ToolCalled {
            agent_id: Uuid::parse_str(&agent_id).unwrap(),
            tool: "read_file".into(),
            args_summary: "path=/foo".into(),
        };
        let event = emit(&pool, &session.id, Some(&agent_id), data).await.unwrap();

        assert_eq!(
            event.metadata.agent_id,
            Some(Uuid::parse_str(&agent_id).unwrap())
        );
    }

    #[tokio::test]
    async fn emit_without_agent_id() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        let data = EventData::PromptSubmitted {
            prompt: "test".into(),
        };
        let event = emit(&pool, &session.id, None, data).await.unwrap();
        assert!(event.metadata.agent_id.is_none());
    }

    #[tokio::test]
    async fn emit_complex_variant() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        let task_id = Uuid::new_v4();
        let data = EventData::TaskCompleted {
            task_id,
            summary: "All tests pass".into(),
            diff_stat: DiffStat {
                files_changed: 3,
                insertions: 42,
                deletions: 10,
            },
        };
        let event = emit(&pool, &session.id, None, data).await.unwrap();

        // Verify the data column has correct nested structure
        let db_events = events_since(&pool, &session.id, "2000-01-01").await.unwrap();
        let stored: serde_json::Value = serde_json::from_str(&db_events[0].data).unwrap();
        assert_eq!(stored["task_id"], task_id.to_string());
        assert_eq!(stored["summary"], "All tests pass");
        assert_eq!(stored["diff_stat"]["files_changed"], 3);
        assert_eq!(stored["diff_stat"]["insertions"], 42);
        assert_eq!(stored["diff_stat"]["deletions"], 10);

        assert_eq!(event.data.kind(), "TaskCompleted");
    }

    #[tokio::test]
    async fn emit_agent_spawned_variant() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();
        let agent_id = Uuid::new_v4();

        let data = EventData::AgentSpawned {
            agent_id,
            role: AgentRole::Implementation,
            model: "claude-sonnet-4-20250514".into(),
            parent_id: None,
        };
        let event = emit(&pool, &session.id, Some(&agent_id.to_string()), data)
            .await
            .unwrap();

        let db_events = events_since(&pool, &session.id, "2000-01-01").await.unwrap();
        let stored: serde_json::Value = serde_json::from_str(&db_events[0].data).unwrap();
        assert_eq!(stored["role"], "implementation");
        assert_eq!(stored["model"], "claude-sonnet-4-20250514");
        assert!(stored["parent_id"].is_null());

        assert_eq!(event.data.kind(), "AgentSpawned");
    }

    #[tokio::test]
    async fn emit_agent_completed_with_token_metrics() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();
        let agent_id = Uuid::new_v4();

        let data = EventData::AgentCompleted {
            agent_id,
            summary: "Done".into(),
            token_usage: TokenMetrics {
                input: 5000,
                output: 1200,
            },
        };
        emit(&pool, &session.id, None, data).await.unwrap();

        let db_events = events_since(&pool, &session.id, "2000-01-01").await.unwrap();
        let stored: serde_json::Value = serde_json::from_str(&db_events[0].data).unwrap();
        assert_eq!(stored["token_usage"]["input"], 5000);
        assert_eq!(stored["token_usage"]["output"], 1200);
    }

    #[tokio::test]
    async fn emit_multiple_events_same_session() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        let events_data = vec![
            EventData::PromptSubmitted {
                prompt: "first".into(),
            },
            EventData::PromptClassified {
                mode: "implement".into(),
                model: "sonnet".into(),
                confidence: 0.95,
            },
            EventData::ModeOverridden {
                from_mode: "implement".into(),
                to_mode: "plan".into(),
            },
        ];

        for data in events_data {
            emit(&pool, &session.id, None, data).await.unwrap();
        }

        let db_events = events_since(&pool, &session.id, "2000-01-01").await.unwrap();
        assert_eq!(db_events.len(), 3);
        assert_eq!(db_events[0].kind, "PromptSubmitted");
        assert_eq!(db_events[1].kind, "PromptClassified");
        assert_eq!(db_events[2].kind, "ModeOverridden");
    }

    #[tokio::test]
    async fn emit_session_id_roundtrips_as_uuid() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        let data = EventData::PromptSubmitted {
            prompt: "test".into(),
        };
        let event = emit(&pool, &session.id, None, data).await.unwrap();

        // session_id in metadata should parse back to the same UUID
        assert_eq!(event.metadata.session_id.to_string(), session.id);
    }
}
