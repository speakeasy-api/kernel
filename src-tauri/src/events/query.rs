use std::collections::HashMap;

use chrono::NaiveDateTime;
use sqlx::SqlitePool;
use tracing::debug;
use uuid::Uuid;

use crate::db::queries as db_queries;
use crate::events::{Event, EventData, EventMetadata};

#[derive(Debug)]
pub enum QueryError {
    Db(sqlx::Error),
    Json(serde_json::Error),
    InvalidUuid(uuid::Error),
    InvalidTimestamp(String),
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "database error: {e}"),
            Self::Json(e) => write!(f, "json error: {e}"),
            Self::InvalidUuid(e) => write!(f, "invalid uuid: {e}"),
            Self::InvalidTimestamp(s) => write!(f, "invalid timestamp: {s}"),
        }
    }
}

impl std::error::Error for QueryError {}

impl From<sqlx::Error> for QueryError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

impl From<serde_json::Error> for QueryError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl From<uuid::Error> for QueryError {
    fn from(e: uuid::Error) -> Self {
        Self::InvalidUuid(e)
    }
}

/// Reconstruct a typed `Event` from a raw DB event row.
///
/// Combines the `kind` and `data` columns back into an `EventData` enum
/// variant using serde's `tag = "kind", content = "data"` format.
fn to_typed(db_event: &crate::db::models::Event) -> Result<Event, QueryError> {
    let data_value: serde_json::Value = serde_json::from_str(&db_event.data)?;
    let combined = serde_json::json!({
        "kind": db_event.kind,
        "data": data_value,
    });
    let data: EventData = serde_json::from_value(combined)?;

    let timestamp = NaiveDateTime::parse_from_str(&db_event.created_at, "%Y-%m-%d %H:%M:%S")
        .map(|ndt| ndt.and_utc())
        .map_err(|_| QueryError::InvalidTimestamp(db_event.created_at.clone()))?;

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

/// All events in a session after the given timestamp, deserialized to typed events.
pub async fn events_since(
    pool: &SqlitePool,
    session_id: &str,
    since: &str,
) -> Result<Vec<Event>, QueryError> {
    debug!(session_id = %session_id, since = %since, "querying events since");
    let db_events = db_queries::events_since(pool, session_id, since).await?;
    let events: Vec<Event> = db_events.iter().map(to_typed).collect::<Result<_, _>>()?;
    debug!(count = events.len(), "query returned events");
    Ok(events)
}

/// All events matching a variant name (the `kind` column) after the given timestamp.
pub async fn events_by_variant(
    pool: &SqlitePool,
    variant: &str,
    since: &str,
) -> Result<Vec<Event>, QueryError> {
    debug!(variant = %variant, since = %since, "querying events by variant");
    let db_events = db_queries::events_by_kind(pool, variant, since).await?;
    let events: Vec<Event> = db_events.iter().map(to_typed).collect::<Result<_, _>>()?;
    debug!(count = events.len(), "query returned events");
    Ok(events)
}

/// Count of events per variant name within a session after the given timestamp.
pub async fn aggregate_by_variant(
    pool: &SqlitePool,
    session_id: &str,
    since: &str,
) -> Result<HashMap<String, u64>, QueryError> {
    debug!(session_id = %session_id, since = %since, "aggregating events by variant");
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind, COUNT(*) FROM events
         WHERE session_id = ?1 AND created_at > ?2
         GROUP BY kind",
    )
    .bind(session_id)
    .bind(since)
    .fetch_all(pool)
    .await?;

    let mut map = HashMap::new();
    for (kind, count) in rows {
        map.insert(kind, count as u64);
    }
    debug!(variant_count = map.len(), "aggregation returned variants");
    Ok(map)
}

/// Ratio of rejected review events to total review events within a time window.
///
/// Review events: PlanAccepted, PlanRejected, DiffAccepted, DiffRejected, HunkRejected.
/// Rejected: PlanRejected, DiffRejected, HunkRejected.
/// Returns 0.0 when there are no review events in the window.
pub async fn rejection_rate(
    pool: &SqlitePool,
    session_id: &str,
    window_seconds: i64,
) -> Result<f32, QueryError> {
    debug!(session_id = %session_id, window_seconds = window_seconds, "computing rejection rate");
    let window_param = format!("-{window_seconds} seconds");

    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM events
         WHERE session_id = ?1
           AND kind IN ('PlanAccepted', 'PlanRejected',
                        'DiffAccepted', 'DiffRejected', 'HunkRejected')
           AND created_at > datetime('now', ?2)",
    )
    .bind(session_id)
    .bind(&window_param)
    .fetch_one(pool)
    .await?;

    if total.0 == 0 {
        return Ok(0.0);
    }

    let rejected: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM events
         WHERE session_id = ?1
           AND kind IN ('PlanRejected', 'DiffRejected', 'HunkRejected')
           AND created_at > datetime('now', ?2)",
    )
    .bind(session_id)
    .bind(&window_param)
    .fetch_one(pool)
    .await?;

    let rate = rejected.0 as f32 / total.0 as f32;
    debug!(
        rate = rate,
        total_reviews = total.0,
        rejected = rejected.0,
        "rejection rate computed"
    );
    Ok(rate)
}

/// Total cost (USD) from all CostIncurred events in a session.
pub async fn cost_total(pool: &SqlitePool, session_id: &str) -> Result<f64, QueryError> {
    debug!(session_id = %session_id, "querying total cost");
    let row: (f64,) = sqlx::query_as(
        "SELECT CAST(COALESCE(SUM(json_extract(data, '$.cost_usd')), 0.0) AS REAL)
         FROM events
         WHERE session_id = ?1 AND kind = 'CostIncurred'",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await?;
    debug!(total_cost = row.0, "total cost computed");
    Ok(row.0)
}

/// Total cost (USD) for a specific task, joining through the agents table
/// to find agents assigned to the task.
pub async fn cost_total_task(pool: &SqlitePool, task_id: &str) -> Result<f64, QueryError> {
    debug!(task_id = %task_id, "querying total cost for task");
    let row: (f64,) = sqlx::query_as(
        "SELECT CAST(COALESCE(SUM(json_extract(e.data, '$.cost_usd')), 0.0) AS REAL)
         FROM events e
         JOIN agents a ON e.agent_id = a.id
         WHERE a.task_id = ?1 AND e.kind = 'CostIncurred'",
    )
    .bind(task_id)
    .fetch_one(pool)
    .await?;
    debug!(task_id = %task_id, total_cost = row.0, "task cost computed");
    Ok(row.0)
}

/// All AgentLooped events in a session after the given timestamp.
pub async fn loop_detections(
    pool: &SqlitePool,
    session_id: &str,
    since: &str,
) -> Result<Vec<Event>, QueryError> {
    debug!(session_id = %session_id, since = %since, "querying loop detections");
    let db_events: Vec<crate::db::models::Event> = sqlx::query_as(
        "SELECT id, kind, session_id, agent_id, data, created_at
         FROM events
         WHERE session_id = ?1 AND kind = 'AgentLooped' AND created_at > ?2
         ORDER BY created_at ASC",
    )
    .bind(session_id)
    .bind(since)
    .fetch_all(pool)
    .await?;

    let events: Vec<Event> = db_events.iter().map(to_typed).collect::<Result<_, _>>()?;
    debug!(count = events.len(), "loop detections returned");
    Ok(events)
}

/// Aggregate raw events into the `stats_rollups` table for the given window period.
///
/// Computes and persists:
/// - `count.<VariantName>` — event count per variant
/// - `cost.usd` — total cost from CostIncurred events
/// - `tokens.total` — total tokens from TokensUsed events
pub async fn rollup_metrics(
    pool: &SqlitePool,
    session_id: &str,
    window_seconds: i64,
) -> Result<(), QueryError> {
    debug!(session_id = %session_id, window_seconds = window_seconds, "rolling up metrics");
    let window_param = format!("-{window_seconds} seconds");

    let row: (String, String) = sqlx::query_as("SELECT datetime('now', ?1), datetime('now')")
        .bind(&window_param)
        .fetch_one(pool)
        .await?;
    let (period_start, period_end) = row;

    // Counts per variant
    let variant_counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind, COUNT(*) FROM events
         WHERE session_id = ?1 AND created_at > ?2 AND created_at <= ?3
         GROUP BY kind",
    )
    .bind(session_id)
    .bind(&period_start)
    .bind(&period_end)
    .fetch_all(pool)
    .await?;

    for (kind, count) in variant_counts {
        db_queries::insert_rollup(
            pool,
            "session",
            Some(session_id),
            &period_start,
            &period_end,
            &format!("count.{kind}"),
            count as f64,
        )
        .await?;
    }

    // Total cost
    let total_cost: (f64,) = sqlx::query_as(
        "SELECT CAST(COALESCE(SUM(json_extract(data, '$.cost_usd')), 0.0) AS REAL)
         FROM events
         WHERE session_id = ?1 AND kind = 'CostIncurred'
           AND created_at > ?2 AND created_at <= ?3",
    )
    .bind(session_id)
    .bind(&period_start)
    .bind(&period_end)
    .fetch_one(pool)
    .await?;
    db_queries::insert_rollup(
        pool,
        "session",
        Some(session_id),
        &period_start,
        &period_end,
        "cost.usd",
        total_cost.0,
    )
    .await?;

    // Total tokens
    let total_tokens: (f64,) = sqlx::query_as(
        "SELECT CAST(COALESCE(
            SUM(json_extract(data, '$.input') + json_extract(data, '$.output')),
            0.0
         ) AS REAL)
         FROM events
         WHERE session_id = ?1 AND kind = 'TokensUsed'
           AND created_at > ?2 AND created_at <= ?3",
    )
    .bind(session_id)
    .bind(&period_start)
    .bind(&period_end)
    .fetch_one(pool)
    .await?;
    db_queries::insert_rollup(
        pool,
        "session",
        Some(session_id),
        &period_start,
        &period_end,
        "tokens.total",
        total_tokens.0,
    )
    .await?;

    debug!(session_id = %session_id, "rollup metrics complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::{create_agent, create_session, create_task, query_rollups};
    use crate::db::test_pool;
    use crate::events::emit::emit;
    use crate::events::DiffStat;

    // ---- events_since ----

    #[tokio::test]
    async fn events_since_returns_typed_events() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        emit(
            &pool,
            &session.id,
            None,
            EventData::PromptSubmitted {
                prompt: "hello".into(),
            },
        )
        .await
        .unwrap();

        let events = events_since(&pool, &session.id, "2000-01-01")
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].data, EventData::PromptSubmitted { .. }));
    }

    #[tokio::test]
    async fn events_since_preserves_metadata() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        let emitted = emit(
            &pool,
            &session.id,
            None,
            EventData::PromptSubmitted {
                prompt: "test".into(),
            },
        )
        .await
        .unwrap();

        let events = events_since(&pool, &session.id, "2000-01-01")
            .await
            .unwrap();
        assert_eq!(events[0].metadata.id, emitted.metadata.id);
        assert_eq!(events[0].metadata.session_id, emitted.metadata.session_id);
    }

    #[tokio::test]
    async fn events_since_deserializes_complex_variant() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();
        let task_id = Uuid::new_v4();

        emit(
            &pool,
            &session.id,
            None,
            EventData::TaskCompleted {
                task_id,
                summary: "done".into(),
                diff_stat: DiffStat {
                    files_changed: 3,
                    insertions: 42,
                    deletions: 10,
                },
            },
        )
        .await
        .unwrap();

        let events = events_since(&pool, &session.id, "2000-01-01")
            .await
            .unwrap();
        match &events[0].data {
            EventData::TaskCompleted {
                task_id: tid,
                summary,
                diff_stat,
            } => {
                assert_eq!(*tid, task_id);
                assert_eq!(summary, "done");
                assert_eq!(diff_stat.files_changed, 3);
            }
            other => panic!("expected TaskCompleted, got {:?}", other),
        }
    }

    // ---- events_by_variant ----

    #[tokio::test]
    async fn events_by_variant_filters_by_kind() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        emit(
            &pool,
            &session.id,
            None,
            EventData::PromptSubmitted { prompt: "a".into() },
        )
        .await
        .unwrap();
        emit(
            &pool,
            &session.id,
            None,
            EventData::PromptClassified {
                mode: "plan".into(),
                model: "sonnet".into(),
                confidence: 0.9,
            },
        )
        .await
        .unwrap();
        emit(
            &pool,
            &session.id,
            None,
            EventData::PromptSubmitted { prompt: "b".into() },
        )
        .await
        .unwrap();

        let events = events_by_variant(&pool, "PromptSubmitted", "2000-01-01")
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert!(events
            .iter()
            .all(|e| matches!(e.data, EventData::PromptSubmitted { .. })));
    }

    // ---- aggregate_by_variant ----

    #[tokio::test]
    async fn aggregate_by_variant_counts() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        for _ in 0..3 {
            emit(
                &pool,
                &session.id,
                None,
                EventData::PromptSubmitted { prompt: "x".into() },
            )
            .await
            .unwrap();
        }
        emit(
            &pool,
            &session.id,
            None,
            EventData::PromptClassified {
                mode: "plan".into(),
                model: "sonnet".into(),
                confidence: 0.9,
            },
        )
        .await
        .unwrap();

        let counts = aggregate_by_variant(&pool, &session.id, "2000-01-01")
            .await
            .unwrap();
        assert_eq!(counts.get("PromptSubmitted"), Some(&3));
        assert_eq!(counts.get("PromptClassified"), Some(&1));
        assert_eq!(counts.get("AgentLooped"), None);
    }

    #[tokio::test]
    async fn aggregate_by_variant_empty_session() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        let counts = aggregate_by_variant(&pool, &session.id, "2000-01-01")
            .await
            .unwrap();
        assert!(counts.is_empty());
    }

    // ---- rejection_rate ----

    #[tokio::test]
    async fn rejection_rate_no_review_events() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        let rate = rejection_rate(&pool, &session.id, 3600).await.unwrap();
        assert_eq!(rate, 0.0);
    }

    #[tokio::test]
    async fn rejection_rate_computes_ratio() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();
        let task_id = Uuid::new_v4();

        // 2 accepted, 2 rejected = 50% rejection rate
        emit(
            &pool,
            &session.id,
            None,
            EventData::PlanAccepted { task_id },
        )
        .await
        .unwrap();
        emit(
            &pool,
            &session.id,
            None,
            EventData::PlanRejected {
                task_id,
                feedback: "needs work".into(),
            },
        )
        .await
        .unwrap();
        emit(
            &pool,
            &session.id,
            None,
            EventData::DiffAccepted {
                task_id,
                branch: "feat".into(),
            },
        )
        .await
        .unwrap();
        emit(
            &pool,
            &session.id,
            None,
            EventData::DiffRejected {
                task_id,
                branch: "feat".into(),
                feedback: "bad".into(),
            },
        )
        .await
        .unwrap();

        let rate = rejection_rate(&pool, &session.id, 3600).await.unwrap();
        assert!((rate - 0.5).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn rejection_rate_all_rejected() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();
        let task_id = Uuid::new_v4();

        emit(
            &pool,
            &session.id,
            None,
            EventData::PlanRejected {
                task_id,
                feedback: "no".into(),
            },
        )
        .await
        .unwrap();
        emit(
            &pool,
            &session.id,
            None,
            EventData::HunkRejected {
                task_id,
                file: "main.rs".into(),
                hunk_index: 0,
                reason: "wrong".into(),
            },
        )
        .await
        .unwrap();

        let rate = rejection_rate(&pool, &session.id, 3600).await.unwrap();
        assert!((rate - 1.0).abs() < f32::EPSILON);
    }

    // ---- cost_total ----

    #[tokio::test]
    async fn cost_total_sums_costs() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();
        let agent_id = Uuid::new_v4();

        emit(
            &pool,
            &session.id,
            Some(&agent_id.to_string()),
            EventData::CostIncurred {
                agent_id,
                model: "sonnet".into(),
                cost_usd: 0.05,
            },
        )
        .await
        .unwrap();
        emit(
            &pool,
            &session.id,
            Some(&agent_id.to_string()),
            EventData::CostIncurred {
                agent_id,
                model: "opus".into(),
                cost_usd: 0.15,
            },
        )
        .await
        .unwrap();

        let total = cost_total(&pool, &session.id).await.unwrap();
        assert!((total - 0.20).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn cost_total_zero_when_no_events() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        let total = cost_total(&pool, &session.id).await.unwrap();
        assert_eq!(total, 0.0);
    }

    // ---- cost_total_task ----

    #[tokio::test]
    async fn cost_total_task_sums_via_agent() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        let task = create_task(
            &pool,
            &session.id,
            "Test task",
            None,
            None,
            "medium",
            "main",
            "abc",
            "main",
        )
        .await
        .unwrap();

        // Create an agent assigned to the task
        let agent = create_agent(
            &pool,
            &session.id,
            None,
            "implementation",
            "sonnet",
            "implement",
        )
        .await
        .unwrap();
        // Assign agent to task
        sqlx::query("UPDATE agents SET task_id = ?1 WHERE id = ?2")
            .bind(&task.id)
            .bind(&agent.id)
            .execute(&pool)
            .await
            .unwrap();

        let agent_uuid = Uuid::parse_str(&agent.id).unwrap();

        emit(
            &pool,
            &session.id,
            Some(&agent.id),
            EventData::CostIncurred {
                agent_id: agent_uuid,
                model: "sonnet".into(),
                cost_usd: 0.10,
            },
        )
        .await
        .unwrap();
        emit(
            &pool,
            &session.id,
            Some(&agent.id),
            EventData::CostIncurred {
                agent_id: agent_uuid,
                model: "sonnet".into(),
                cost_usd: 0.25,
            },
        )
        .await
        .unwrap();

        let total = cost_total_task(&pool, &task.id).await.unwrap();
        assert!((total - 0.35).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn cost_total_task_excludes_other_tasks() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        let task1 = create_task(
            &pool,
            &session.id,
            "Task 1",
            None,
            None,
            "medium",
            "main",
            "abc",
            "main",
        )
        .await
        .unwrap();
        let task2 = create_task(
            &pool,
            &session.id,
            "Task 2",
            None,
            None,
            "medium",
            "main",
            "abc",
            "main",
        )
        .await
        .unwrap();

        let agent1 = create_agent(&pool, &session.id, None, "impl", "sonnet", "implement")
            .await
            .unwrap();
        sqlx::query("UPDATE agents SET task_id = ?1 WHERE id = ?2")
            .bind(&task1.id)
            .bind(&agent1.id)
            .execute(&pool)
            .await
            .unwrap();

        let agent2 = create_agent(&pool, &session.id, None, "impl", "sonnet", "implement")
            .await
            .unwrap();
        sqlx::query("UPDATE agents SET task_id = ?1 WHERE id = ?2")
            .bind(&task2.id)
            .bind(&agent2.id)
            .execute(&pool)
            .await
            .unwrap();

        let agent1_uuid = Uuid::parse_str(&agent1.id).unwrap();
        let agent2_uuid = Uuid::parse_str(&agent2.id).unwrap();

        emit(
            &pool,
            &session.id,
            Some(&agent1.id),
            EventData::CostIncurred {
                agent_id: agent1_uuid,
                model: "sonnet".into(),
                cost_usd: 1.00,
            },
        )
        .await
        .unwrap();
        emit(
            &pool,
            &session.id,
            Some(&agent2.id),
            EventData::CostIncurred {
                agent_id: agent2_uuid,
                model: "sonnet".into(),
                cost_usd: 2.00,
            },
        )
        .await
        .unwrap();

        assert!((cost_total_task(&pool, &task1.id).await.unwrap() - 1.00).abs() < f64::EPSILON);
        assert!((cost_total_task(&pool, &task2.id).await.unwrap() - 2.00).abs() < f64::EPSILON);
    }

    // ---- loop_detections ----

    #[tokio::test]
    async fn loop_detections_returns_agent_looped() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();
        let agent_id = Uuid::new_v4();

        emit(
            &pool,
            &session.id,
            Some(&agent_id.to_string()),
            EventData::AgentLooped {
                agent_id,
                repeated_tool: "read_file".into(),
                count: 5,
            },
        )
        .await
        .unwrap();
        // Non-loop event should be excluded
        emit(
            &pool,
            &session.id,
            None,
            EventData::PromptSubmitted {
                prompt: "hi".into(),
            },
        )
        .await
        .unwrap();

        let loops = loop_detections(&pool, &session.id, "2000-01-01")
            .await
            .unwrap();
        assert_eq!(loops.len(), 1);
        match &loops[0].data {
            EventData::AgentLooped {
                repeated_tool,
                count,
                ..
            } => {
                assert_eq!(repeated_tool, "read_file");
                assert_eq!(*count, 5);
            }
            other => panic!("expected AgentLooped, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn loop_detections_empty_when_none() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        emit(
            &pool,
            &session.id,
            None,
            EventData::PromptSubmitted {
                prompt: "hi".into(),
            },
        )
        .await
        .unwrap();

        let loops = loop_detections(&pool, &session.id, "2000-01-01")
            .await
            .unwrap();
        assert!(loops.is_empty());
    }

    // ---- rollup_metrics ----

    #[tokio::test]
    async fn rollup_metrics_persists_counts() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();
        let agent_id = Uuid::new_v4();

        for _ in 0..3 {
            emit(
                &pool,
                &session.id,
                None,
                EventData::PromptSubmitted { prompt: "x".into() },
            )
            .await
            .unwrap();
        }
        emit(
            &pool,
            &session.id,
            Some(&agent_id.to_string()),
            EventData::CostIncurred {
                agent_id,
                model: "sonnet".into(),
                cost_usd: 0.50,
            },
        )
        .await
        .unwrap();
        emit(
            &pool,
            &session.id,
            Some(&agent_id.to_string()),
            EventData::TokensUsed {
                agent_id,
                model: "sonnet".into(),
                input: 1000,
                output: 500,
            },
        )
        .await
        .unwrap();

        rollup_metrics(&pool, &session.id, 3600).await.unwrap();

        // Verify variant counts
        let prompt_counts = query_rollups(
            &pool,
            "session",
            Some(&session.id),
            "count.PromptSubmitted",
            "2000-01-01",
        )
        .await
        .unwrap();
        assert_eq!(prompt_counts.len(), 1);
        assert!((prompt_counts[0].value - 3.0).abs() < f64::EPSILON);

        // Verify cost rollup
        let cost_rollups = query_rollups(
            &pool,
            "session",
            Some(&session.id),
            "cost.usd",
            "2000-01-01",
        )
        .await
        .unwrap();
        assert_eq!(cost_rollups.len(), 1);
        assert!((cost_rollups[0].value - 0.50).abs() < f64::EPSILON);

        // Verify token rollup
        let token_rollups = query_rollups(
            &pool,
            "session",
            Some(&session.id),
            "tokens.total",
            "2000-01-01",
        )
        .await
        .unwrap();
        assert_eq!(token_rollups.len(), 1);
        assert!((token_rollups[0].value - 1500.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn rollup_metrics_zero_when_no_events() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test").await.unwrap();

        rollup_metrics(&pool, &session.id, 3600).await.unwrap();

        let cost_rollups = query_rollups(
            &pool,
            "session",
            Some(&session.id),
            "cost.usd",
            "2000-01-01",
        )
        .await
        .unwrap();
        assert_eq!(cost_rollups.len(), 1);
        assert_eq!(cost_rollups[0].value, 0.0);

        let token_rollups = query_rollups(
            &pool,
            "session",
            Some(&session.id),
            "tokens.total",
            "2000-01-01",
        )
        .await
        .unwrap();
        assert_eq!(token_rollups.len(), 1);
        assert_eq!(token_rollups[0].value, 0.0);
    }
}
