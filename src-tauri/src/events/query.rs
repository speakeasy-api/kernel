use std::collections::HashMap;

use chrono::NaiveDateTime;
use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::db::queries as db_queries;
use crate::events::{Event, EventData, EventMetadata};

#[derive(Debug)]
pub enum QueryError {
    Db(rusqlite::Error),
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

impl From<rusqlite::Error> for QueryError {
    fn from(e: rusqlite::Error) -> Self {
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

/// Map a rusqlite row (id, kind, session_id, agent_id, data, created_at) to a db Event.
fn row_to_db_event(row: &rusqlite::Row) -> Result<crate::db::models::Event, rusqlite::Error> {
    Ok(crate::db::models::Event {
        id: row.get(0)?,
        kind: row.get(1)?,
        session_id: row.get(2)?,
        agent_id: row.get(3)?,
        data: row.get(4)?,
        created_at: row.get(5)?,
    })
}

/// All events in a session after the given timestamp, deserialized to typed events.
pub fn events_since(
    conn: &Connection,
    session_id: &str,
    since: &str,
) -> Result<Vec<Event>, QueryError> {
    let db_events = db_queries::events_since(conn, session_id, since)?;
    db_events.iter().map(to_typed).collect()
}

/// All events matching a variant name (the `kind` column) after the given timestamp.
pub fn events_by_variant(
    conn: &Connection,
    variant: &str,
    since: &str,
) -> Result<Vec<Event>, QueryError> {
    let db_events = db_queries::events_by_kind(conn, variant, since)?;
    db_events.iter().map(to_typed).collect()
}

/// Count of events per variant name within a session after the given timestamp.
pub fn aggregate_by_variant(
    conn: &Connection,
    session_id: &str,
    since: &str,
) -> Result<HashMap<String, u64>, QueryError> {
    let mut stmt = conn.prepare(
        "SELECT kind, COUNT(*) FROM events
         WHERE session_id = ?1 AND created_at > ?2
         GROUP BY kind",
    )?;
    let rows = stmt.query_map(params![session_id, since], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
    })?;
    let mut map = HashMap::new();
    for row in rows {
        let (kind, count) = row?;
        map.insert(kind, count);
    }
    Ok(map)
}

/// Ratio of rejected review events to total review events within a time window.
///
/// Review events: PlanAccepted, PlanRejected, DiffAccepted, DiffRejected, HunkRejected.
/// Rejected: PlanRejected, DiffRejected, HunkRejected.
/// Returns 0.0 when there are no review events in the window.
pub fn rejection_rate(
    conn: &Connection,
    session_id: &str,
    window_seconds: i64,
) -> Result<f32, QueryError> {
    let window_param = format!("-{window_seconds} seconds");

    let total: u64 = conn.query_row(
        "SELECT COUNT(*) FROM events
         WHERE session_id = ?1
           AND kind IN ('PlanAccepted', 'PlanRejected',
                        'DiffAccepted', 'DiffRejected', 'HunkRejected')
           AND created_at > datetime('now', ?2)",
        params![session_id, window_param],
        |row| row.get(0),
    )?;

    if total == 0 {
        return Ok(0.0);
    }

    let rejected: u64 = conn.query_row(
        "SELECT COUNT(*) FROM events
         WHERE session_id = ?1
           AND kind IN ('PlanRejected', 'DiffRejected', 'HunkRejected')
           AND created_at > datetime('now', ?2)",
        params![session_id, window_param],
        |row| row.get(0),
    )?;

    Ok(rejected as f32 / total as f32)
}

/// Total cost (USD) from all CostIncurred events in a session.
pub fn cost_total(conn: &Connection, session_id: &str) -> Result<f64, QueryError> {
    let total: f64 = conn.query_row(
        "SELECT COALESCE(SUM(json_extract(data, '$.cost_usd')), 0.0)
         FROM events
         WHERE session_id = ?1 AND kind = 'CostIncurred'",
        params![session_id],
        |row| row.get(0),
    )?;
    Ok(total)
}

/// Total cost (USD) for a specific task, joining through the agents table
/// to find agents assigned to the task.
pub fn cost_total_task(conn: &Connection, task_id: &str) -> Result<f64, QueryError> {
    let total: f64 = conn.query_row(
        "SELECT COALESCE(SUM(json_extract(e.data, '$.cost_usd')), 0.0)
         FROM events e
         JOIN agents a ON e.agent_id = a.id
         WHERE a.task_id = ?1 AND e.kind = 'CostIncurred'",
        params![task_id],
        |row| row.get(0),
    )?;
    Ok(total)
}

/// All AgentLooped events in a session after the given timestamp.
pub fn loop_detections(
    conn: &Connection,
    session_id: &str,
    since: &str,
) -> Result<Vec<Event>, QueryError> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, session_id, agent_id, data, created_at
         FROM events
         WHERE session_id = ?1 AND kind = 'AgentLooped' AND created_at > ?2
         ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(params![session_id, since], row_to_db_event)?;
    let mut events = Vec::new();
    for row in rows {
        events.push(to_typed(&row?)?);
    }
    Ok(events)
}

/// Aggregate raw events into the `stats_rollups` table for the given window period.
///
/// Computes and persists:
/// - `count.<VariantName>` — event count per variant
/// - `cost.usd` — total cost from CostIncurred events
/// - `tokens.total` — total tokens from TokensUsed events
pub fn rollup_metrics(
    conn: &Connection,
    session_id: &str,
    window_seconds: i64,
) -> Result<(), QueryError> {
    let window_param = format!("-{window_seconds} seconds");

    let (period_start, period_end): (String, String) = conn.query_row(
        "SELECT datetime('now', ?1), datetime('now')",
        params![window_param],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    // Counts per variant
    let mut stmt = conn.prepare(
        "SELECT kind, COUNT(*) FROM events
         WHERE session_id = ?1 AND created_at > ?2 AND created_at <= ?3
         GROUP BY kind",
    )?;
    let variant_counts = stmt.query_map(params![session_id, period_start, period_end], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
    })?;
    for row in variant_counts {
        let (kind, count) = row?;
        db_queries::insert_rollup(
            conn,
            "session",
            Some(session_id),
            &period_start,
            &period_end,
            &format!("count.{kind}"),
            count,
        )?;
    }

    // Total cost
    let total_cost: f64 = conn.query_row(
        "SELECT COALESCE(SUM(json_extract(data, '$.cost_usd')), 0.0)
         FROM events
         WHERE session_id = ?1 AND kind = 'CostIncurred'
           AND created_at > ?2 AND created_at <= ?3",
        params![session_id, period_start, period_end],
        |row| row.get(0),
    )?;
    db_queries::insert_rollup(
        conn,
        "session",
        Some(session_id),
        &period_start,
        &period_end,
        "cost.usd",
        total_cost,
    )?;

    // Total tokens
    let total_tokens: f64 = conn.query_row(
        "SELECT COALESCE(
            SUM(json_extract(data, '$.input') + json_extract(data, '$.output')),
            0.0
         )
         FROM events
         WHERE session_id = ?1 AND kind = 'TokensUsed'
           AND created_at > ?2 AND created_at <= ?3",
        params![session_id, period_start, period_end],
        |row| row.get(0),
    )?;
    db_queries::insert_rollup(
        conn,
        "session",
        Some(session_id),
        &period_start,
        &period_end,
        "tokens.total",
        total_tokens,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations;
    use crate::db::models::Priority;
    use crate::db::queries::{create_agent, create_session, create_task, query_rollups};
    use crate::events::emit::emit;
    use crate::events::DiffStat;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        migrations::run_migrations(&conn).unwrap();
        conn
    }

    // ---- events_since ----

    #[test]
    fn events_since_returns_typed_events() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();

        emit(
            &conn,
            &session.id,
            None,
            EventData::PromptSubmitted {
                prompt: "hello".into(),
            },
        )
        .unwrap();

        let events = events_since(&conn, &session.id, "2000-01-01").unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].data, EventData::PromptSubmitted { .. }));
    }

    #[test]
    fn events_since_preserves_metadata() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();

        let emitted = emit(
            &conn,
            &session.id,
            None,
            EventData::PromptSubmitted {
                prompt: "test".into(),
            },
        )
        .unwrap();

        let events = events_since(&conn, &session.id, "2000-01-01").unwrap();
        assert_eq!(events[0].metadata.id, emitted.metadata.id);
        assert_eq!(events[0].metadata.session_id, emitted.metadata.session_id);
    }

    #[test]
    fn events_since_deserializes_complex_variant() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();
        let task_id = Uuid::new_v4();

        emit(
            &conn,
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
        .unwrap();

        let events = events_since(&conn, &session.id, "2000-01-01").unwrap();
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

    #[test]
    fn events_by_variant_filters_by_kind() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();

        emit(
            &conn,
            &session.id,
            None,
            EventData::PromptSubmitted { prompt: "a".into() },
        )
        .unwrap();
        emit(
            &conn,
            &session.id,
            None,
            EventData::PromptClassified {
                mode: "plan".into(),
                model: "sonnet".into(),
                confidence: 0.9,
            },
        )
        .unwrap();
        emit(
            &conn,
            &session.id,
            None,
            EventData::PromptSubmitted { prompt: "b".into() },
        )
        .unwrap();

        let events = events_by_variant(&conn, "PromptSubmitted", "2000-01-01").unwrap();
        assert_eq!(events.len(), 2);
        assert!(events
            .iter()
            .all(|e| matches!(e.data, EventData::PromptSubmitted { .. })));
    }

    // ---- aggregate_by_variant ----

    #[test]
    fn aggregate_by_variant_counts() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();

        for _ in 0..3 {
            emit(
                &conn,
                &session.id,
                None,
                EventData::PromptSubmitted { prompt: "x".into() },
            )
            .unwrap();
        }
        emit(
            &conn,
            &session.id,
            None,
            EventData::PromptClassified {
                mode: "plan".into(),
                model: "sonnet".into(),
                confidence: 0.9,
            },
        )
        .unwrap();

        let counts = aggregate_by_variant(&conn, &session.id, "2000-01-01").unwrap();
        assert_eq!(counts.get("PromptSubmitted"), Some(&3));
        assert_eq!(counts.get("PromptClassified"), Some(&1));
        assert_eq!(counts.get("AgentLooped"), None);
    }

    #[test]
    fn aggregate_by_variant_empty_session() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();

        let counts = aggregate_by_variant(&conn, &session.id, "2000-01-01").unwrap();
        assert!(counts.is_empty());
    }

    // ---- rejection_rate ----

    #[test]
    fn rejection_rate_no_review_events() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();

        let rate = rejection_rate(&conn, &session.id, 3600).unwrap();
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn rejection_rate_computes_ratio() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();
        let task_id = Uuid::new_v4();

        // 2 accepted, 2 rejected = 50% rejection rate
        emit(
            &conn,
            &session.id,
            None,
            EventData::PlanAccepted { task_id },
        )
        .unwrap();
        emit(
            &conn,
            &session.id,
            None,
            EventData::PlanRejected {
                task_id,
                feedback: "needs work".into(),
            },
        )
        .unwrap();
        emit(
            &conn,
            &session.id,
            None,
            EventData::DiffAccepted {
                task_id,
                branch: "feat".into(),
            },
        )
        .unwrap();
        emit(
            &conn,
            &session.id,
            None,
            EventData::DiffRejected {
                task_id,
                branch: "feat".into(),
                feedback: "bad".into(),
            },
        )
        .unwrap();

        let rate = rejection_rate(&conn, &session.id, 3600).unwrap();
        assert!((rate - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn rejection_rate_all_rejected() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();
        let task_id = Uuid::new_v4();

        emit(
            &conn,
            &session.id,
            None,
            EventData::PlanRejected {
                task_id,
                feedback: "no".into(),
            },
        )
        .unwrap();
        emit(
            &conn,
            &session.id,
            None,
            EventData::HunkRejected {
                task_id,
                file: "main.rs".into(),
                hunk_index: 0,
                reason: "wrong".into(),
            },
        )
        .unwrap();

        let rate = rejection_rate(&conn, &session.id, 3600).unwrap();
        assert!((rate - 1.0).abs() < f32::EPSILON);
    }

    // ---- cost_total ----

    #[test]
    fn cost_total_sums_costs() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();
        let agent_id = Uuid::new_v4();

        emit(
            &conn,
            &session.id,
            Some(&agent_id.to_string()),
            EventData::CostIncurred {
                agent_id,
                model: "sonnet".into(),
                cost_usd: 0.05,
            },
        )
        .unwrap();
        emit(
            &conn,
            &session.id,
            Some(&agent_id.to_string()),
            EventData::CostIncurred {
                agent_id,
                model: "opus".into(),
                cost_usd: 0.15,
            },
        )
        .unwrap();

        let total = cost_total(&conn, &session.id).unwrap();
        assert!((total - 0.20).abs() < f64::EPSILON);
    }

    #[test]
    fn cost_total_zero_when_no_events() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();

        let total = cost_total(&conn, &session.id).unwrap();
        assert_eq!(total, 0.0);
    }

    // ---- cost_total_task ----

    #[test]
    fn cost_total_task_sums_via_agent() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();

        let task = create_task(
            &conn,
            &session.id,
            "Test task",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();

        // Create an agent assigned to the task
        let agent = create_agent(
            &conn,
            &session.id,
            None,
            "implementation",
            "sonnet",
            "implement",
        )
        .unwrap();
        // Assign agent to task
        conn.execute(
            "UPDATE agents SET task_id = ?1 WHERE id = ?2",
            params![task.id, agent.id],
        )
        .unwrap();

        let agent_uuid = Uuid::parse_str(&agent.id).unwrap();

        emit(
            &conn,
            &session.id,
            Some(&agent.id),
            EventData::CostIncurred {
                agent_id: agent_uuid,
                model: "sonnet".into(),
                cost_usd: 0.10,
            },
        )
        .unwrap();
        emit(
            &conn,
            &session.id,
            Some(&agent.id),
            EventData::CostIncurred {
                agent_id: agent_uuid,
                model: "sonnet".into(),
                cost_usd: 0.25,
            },
        )
        .unwrap();

        let total = cost_total_task(&conn, &task.id).unwrap();
        assert!((total - 0.35).abs() < f64::EPSILON);
    }

    #[test]
    fn cost_total_task_excludes_other_tasks() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();

        let task1 = create_task(
            &conn,
            &session.id,
            "Task 1",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();
        let task2 = create_task(
            &conn,
            &session.id,
            "Task 2",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();

        let agent1 = create_agent(&conn, &session.id, None, "impl", "sonnet", "implement").unwrap();
        conn.execute(
            "UPDATE agents SET task_id = ?1 WHERE id = ?2",
            params![task1.id, agent1.id],
        )
        .unwrap();

        let agent2 = create_agent(&conn, &session.id, None, "impl", "sonnet", "implement").unwrap();
        conn.execute(
            "UPDATE agents SET task_id = ?1 WHERE id = ?2",
            params![task2.id, agent2.id],
        )
        .unwrap();

        let agent1_uuid = Uuid::parse_str(&agent1.id).unwrap();
        let agent2_uuid = Uuid::parse_str(&agent2.id).unwrap();

        emit(
            &conn,
            &session.id,
            Some(&agent1.id),
            EventData::CostIncurred {
                agent_id: agent1_uuid,
                model: "sonnet".into(),
                cost_usd: 1.00,
            },
        )
        .unwrap();
        emit(
            &conn,
            &session.id,
            Some(&agent2.id),
            EventData::CostIncurred {
                agent_id: agent2_uuid,
                model: "sonnet".into(),
                cost_usd: 2.00,
            },
        )
        .unwrap();

        assert!((cost_total_task(&conn, &task1.id).unwrap() - 1.00).abs() < f64::EPSILON);
        assert!((cost_total_task(&conn, &task2.id).unwrap() - 2.00).abs() < f64::EPSILON);
    }

    // ---- loop_detections ----

    #[test]
    fn loop_detections_returns_agent_looped() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();
        let agent_id = Uuid::new_v4();

        emit(
            &conn,
            &session.id,
            Some(&agent_id.to_string()),
            EventData::AgentLooped {
                agent_id,
                repeated_tool: "read_file".into(),
                count: 5,
            },
        )
        .unwrap();
        // Non-loop event should be excluded
        emit(
            &conn,
            &session.id,
            None,
            EventData::PromptSubmitted {
                prompt: "hi".into(),
            },
        )
        .unwrap();

        let loops = loop_detections(&conn, &session.id, "2000-01-01").unwrap();
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

    #[test]
    fn loop_detections_empty_when_none() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();

        emit(
            &conn,
            &session.id,
            None,
            EventData::PromptSubmitted {
                prompt: "hi".into(),
            },
        )
        .unwrap();

        let loops = loop_detections(&conn, &session.id, "2000-01-01").unwrap();
        assert!(loops.is_empty());
    }

    // ---- rollup_metrics ----

    #[test]
    fn rollup_metrics_persists_counts() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();
        let agent_id = Uuid::new_v4();

        for _ in 0..3 {
            emit(
                &conn,
                &session.id,
                None,
                EventData::PromptSubmitted { prompt: "x".into() },
            )
            .unwrap();
        }
        emit(
            &conn,
            &session.id,
            Some(&agent_id.to_string()),
            EventData::CostIncurred {
                agent_id,
                model: "sonnet".into(),
                cost_usd: 0.50,
            },
        )
        .unwrap();
        emit(
            &conn,
            &session.id,
            Some(&agent_id.to_string()),
            EventData::TokensUsed {
                agent_id,
                model: "sonnet".into(),
                input: 1000,
                output: 500,
            },
        )
        .unwrap();

        rollup_metrics(&conn, &session.id, 3600).unwrap();

        // Verify variant counts
        let prompt_counts = query_rollups(
            &conn,
            "session",
            Some(&session.id),
            "count.PromptSubmitted",
            "2000-01-01",
        )
        .unwrap();
        assert_eq!(prompt_counts.len(), 1);
        assert!((prompt_counts[0].value - 3.0).abs() < f64::EPSILON);

        // Verify cost rollup
        let cost_rollups = query_rollups(
            &conn,
            "session",
            Some(&session.id),
            "cost.usd",
            "2000-01-01",
        )
        .unwrap();
        assert_eq!(cost_rollups.len(), 1);
        assert!((cost_rollups[0].value - 0.50).abs() < f64::EPSILON);

        // Verify token rollup
        let token_rollups = query_rollups(
            &conn,
            "session",
            Some(&session.id),
            "tokens.total",
            "2000-01-01",
        )
        .unwrap();
        assert_eq!(token_rollups.len(), 1);
        assert!((token_rollups[0].value - 1500.0).abs() < f64::EPSILON);
    }

    #[test]
    fn rollup_metrics_zero_when_no_events() {
        let conn = setup();
        let session = create_session(&conn, "/tmp/test").unwrap();

        rollup_metrics(&conn, &session.id, 3600).unwrap();

        let cost_rollups = query_rollups(
            &conn,
            "session",
            Some(&session.id),
            "cost.usd",
            "2000-01-01",
        )
        .unwrap();
        assert_eq!(cost_rollups.len(), 1);
        assert_eq!(cost_rollups[0].value, 0.0);

        let token_rollups = query_rollups(
            &conn,
            "session",
            Some(&session.id),
            "tokens.total",
            "2000-01-01",
        )
        .unwrap();
        assert_eq!(token_rollups.len(), 1);
        assert_eq!(token_rollups[0].value, 0.0);
    }
}
