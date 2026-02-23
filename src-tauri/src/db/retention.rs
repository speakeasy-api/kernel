use sqlx::SqlitePool;

use super::models::RetentionReport;

pub async fn run_retention(
    pool: &SqlitePool,
    raw_ttl_days: u32,
) -> Result<RetentionReport, sqlx::Error> {
    let cutoff = format!("-{raw_ttl_days} days");

    let eligible_session_clause =
        "session_id NOT IN (SELECT DISTINCT session_id FROM tasks WHERE status != 'done')";

    // 1. Delete old events
    let events_result = sqlx::query(&format!(
        "DELETE FROM events
         WHERE created_at < datetime('now', ?1)
           AND {eligible_session_clause}"
    ))
    .bind(&cutoff)
    .execute(pool)
    .await?;

    // 2. Delete old agents (null out parent_agent_id first)
    sqlx::query(&format!(
        "UPDATE agents SET parent_agent_id = NULL
         WHERE created_at < datetime('now', ?1)
           AND {eligible_session_clause}"
    ))
    .bind(&cutoff)
    .execute(pool)
    .await?;

    let agents_result = sqlx::query(&format!(
        "DELETE FROM agents
         WHERE created_at < datetime('now', ?1)
           AND {eligible_session_clause}"
    ))
    .bind(&cutoff)
    .execute(pool)
    .await?;

    // 3. Delete old tasks (clean up task_deps and parent_task FKs first)
    sqlx::query(&format!(
        "DELETE FROM task_deps
         WHERE task_id IN (
             SELECT id FROM tasks
             WHERE created_at < datetime('now', ?1)
               AND {eligible_session_clause}
         ) OR depends_on_task_id IN (
             SELECT id FROM tasks
             WHERE created_at < datetime('now', ?1)
               AND {eligible_session_clause}
         )"
    ))
    .bind(&cutoff)
    .execute(pool)
    .await?;

    sqlx::query(&format!(
        "UPDATE tasks SET parent_task = NULL
         WHERE created_at < datetime('now', ?1)
           AND {eligible_session_clause}"
    ))
    .bind(&cutoff)
    .execute(pool)
    .await?;

    let tasks_result = sqlx::query(&format!(
        "DELETE FROM tasks
         WHERE created_at < datetime('now', ?1)
           AND {eligible_session_clause}"
    ))
    .bind(&cutoff)
    .execute(pool)
    .await?;

    // 4. Delete old sessions with no remaining children
    let sessions_result = sqlx::query(
        "DELETE FROM sessions
         WHERE created_at < datetime('now', ?1)
           AND id NOT IN (SELECT DISTINCT session_id FROM tasks)
           AND id NOT IN (SELECT DISTINCT session_id FROM events)
           AND id NOT IN (SELECT DISTINCT session_id FROM agents)",
    )
    .bind(&cutoff)
    .execute(pool)
    .await?;

    Ok(RetentionReport {
        events_deleted: events_result.rows_affected(),
        agents_deleted: agents_result.rows_affected(),
        tasks_deleted: tasks_result.rows_affected(),
        sessions_deleted: sessions_result.rows_affected(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::queries::*;
    use super::super::test_pool;
    use super::*;
    use sqlx::SqlitePool;

    async fn age_event(pool: &SqlitePool, event_id: &str, days_ago: u32) {
        sqlx::query("UPDATE events SET created_at = datetime('now', ?1) WHERE id = ?2")
            .bind(format!("-{days_ago} days"))
            .bind(event_id)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn age_agent(pool: &SqlitePool, agent_id: &str, days_ago: u32) {
        sqlx::query("UPDATE agents SET created_at = datetime('now', ?1) WHERE id = ?2")
            .bind(format!("-{days_ago} days"))
            .bind(agent_id)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn age_session(pool: &SqlitePool, session_id: &str, days_ago: u32) {
        sqlx::query("UPDATE sessions SET created_at = datetime('now', ?1) WHERE id = ?2")
            .bind(format!("-{days_ago} days"))
            .bind(session_id)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn age_task(pool: &SqlitePool, task_id: &str, days_ago: u32) {
        sqlx::query(
            "UPDATE tasks SET created_at = datetime('now', ?1), updated_at = datetime('now', ?1) WHERE id = ?2",
        )
        .bind(format!("-{days_ago} days"))
        .bind(task_id)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_deletes_old_events_and_agents() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        let e1 = insert_event(&pool, &session.id, None, "prompt_submitted", "{}")
            .await
            .unwrap();
        let e2 = insert_event(&pool, &session.id, None, "tool_called", "{}")
            .await
            .unwrap();
        age_event(&pool, &e1.id, 60).await;
        age_event(&pool, &e2.id, 60).await;

        let a1 = create_agent(
            &pool,
            &session.id,
            None,
            "research",
            "claude-haiku-4-5-20251001",
            "research",
        )
        .await
        .unwrap();
        age_agent(&pool, &a1.id, 60).await;

        let report = run_retention(&pool, 30).await.unwrap();
        assert_eq!(report.events_deleted, 2);
        assert_eq!(report.agents_deleted, 1);
        assert_eq!(report.tasks_deleted, 0);
    }

    #[tokio::test]
    async fn test_preserves_recent_data() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();

        insert_event(&pool, &session.id, None, "prompt_submitted", "{}")
            .await
            .unwrap();
        create_agent(
            &pool,
            &session.id,
            None,
            "research",
            "claude-haiku-4-5-20251001",
            "research",
        )
        .await
        .unwrap();

        let report = run_retention(&pool, 30).await.unwrap();
        assert_eq!(report.events_deleted, 0);
        assert_eq!(report.agents_deleted, 0);
        assert_eq!(report.tasks_deleted, 0);
        assert_eq!(report.sessions_deleted, 0);
    }

    #[tokio::test]
    async fn test_noop_on_empty_database() {
        let pool = test_pool().await;

        let report = run_retention(&pool, 30).await.unwrap();
        assert_eq!(report.events_deleted, 0);
        assert_eq!(report.agents_deleted, 0);
        assert_eq!(report.tasks_deleted, 0);
        assert_eq!(report.sessions_deleted, 0);
    }

    #[tokio::test]
    async fn test_deletes_old_tasks_and_sessions() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();
        age_session(&pool, &session.id, 60).await;

        let task = create_task(
            &pool,
            &session.id,
            "Old done task",
            None,
            None,
            "medium",
            "main",
            "abc",
            "main",
        )
        .await
        .unwrap();
        update_task_status(&pool, &task.id, "done", Some("success"), None)
            .await
            .unwrap();
        age_task(&pool, &task.id, 60).await;

        let report = run_retention(&pool, 30).await.unwrap();
        assert_eq!(report.tasks_deleted, 1);
        assert_eq!(report.sessions_deleted, 1);
    }

    #[tokio::test]
    async fn test_deletes_old_empty_session() {
        let pool = test_pool().await;
        let session = create_session(&pool, "/tmp/test-project").await.unwrap();
        age_session(&pool, &session.id, 60).await;

        let report = run_retention(&pool, 30).await.unwrap();
        assert_eq!(report.sessions_deleted, 1);
    }
}
