use rusqlite::{params, Connection};

use super::models::RetentionReport;

/// Evicts raw operational rows older than the configured TTL.
///
/// - Deletes events older than `raw_ttl_days`
/// - Deletes agents older than `raw_ttl_days`
/// - Deletes old tasks and sessions once all tasks are done and data is cleaned up
/// - Never deletes stats_rollups
/// - Preserves events/agents belonging to sessions that still have active tasks
///   (any task status other than 'done')
pub fn run_retention(
    conn: &Connection,
    raw_ttl_days: u32,
) -> Result<RetentionReport, rusqlite::Error> {
    let cutoff = format!("-{raw_ttl_days} days");

    let tx = conn.unchecked_transaction()?;

    // Sessions with active (non-done) tasks are protected from all cleanup.
    let eligible_session_clause =
        "session_id NOT IN (SELECT DISTINCT session_id FROM tasks WHERE status != 'done')";

    // 1. Delete old events. Nothing references events via FK.
    let events_deleted = tx.execute(
        &format!(
            "DELETE FROM events
             WHERE created_at < datetime('now', ?1)
               AND {eligible_session_clause}"
        ),
        params![cutoff],
    )?;

    // 2. Delete old agents. Null out self-referential parent_agent_id first
    //    so parent+child pairs in the deletion set don't cause FK violations.
    tx.execute(
        &format!(
            "UPDATE agents SET parent_agent_id = NULL
             WHERE created_at < datetime('now', ?1)
               AND {eligible_session_clause}"
        ),
        params![cutoff],
    )?;
    let agents_deleted = tx.execute(
        &format!(
            "DELETE FROM agents
             WHERE created_at < datetime('now', ?1)
               AND {eligible_session_clause}"
        ),
        params![cutoff],
    )?;

    // 3. Delete old tasks. Clean up task_deps and parent_id FKs first.
    tx.execute(
        &format!(
            "DELETE FROM task_deps
             WHERE task_id IN (
                 SELECT id FROM tasks
                 WHERE created_at < datetime('now', ?1)
                   AND {eligible_session_clause}
             ) OR depends_on IN (
                 SELECT id FROM tasks
                 WHERE created_at < datetime('now', ?1)
                   AND {eligible_session_clause}
             )"
        ),
        params![cutoff],
    )?;
    tx.execute(
        &format!(
            "UPDATE tasks SET parent_id = NULL
             WHERE created_at < datetime('now', ?1)
               AND {eligible_session_clause}"
        ),
        params![cutoff],
    )?;
    let tasks_deleted = tx.execute(
        &format!(
            "DELETE FROM tasks
             WHERE created_at < datetime('now', ?1)
               AND {eligible_session_clause}"
        ),
        params![cutoff],
    )?;

    // 4. Delete old sessions with no remaining children.
    let sessions_deleted = tx.execute(
        "DELETE FROM sessions
         WHERE created_at < datetime('now', ?1)
           AND id NOT IN (SELECT DISTINCT session_id FROM tasks)
           AND id NOT IN (SELECT DISTINCT session_id FROM events)
           AND id NOT IN (SELECT DISTINCT session_id FROM agents)",
        params![cutoff],
    )?;

    tx.commit()?;

    Ok(RetentionReport {
        events_deleted,
        agents_deleted,
        tasks_deleted,
        sessions_deleted,
    })
}

#[cfg(test)]
mod tests {
    use super::super::migrations;
    use super::super::models::*;
    use super::super::queries::*;
    use super::*;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        migrations::run_migrations(&conn).unwrap();
        conn
    }

    fn create_test_session(conn: &Connection) -> Session {
        create_session(conn, "/tmp/test-project").unwrap()
    }

    fn age_event(conn: &Connection, event_id: &str, days_ago: u32) {
        conn.execute(
            "UPDATE events SET created_at = datetime('now', ?1) WHERE id = ?2",
            params![format!("-{days_ago} days"), event_id],
        )
        .unwrap();
    }

    fn age_agent(conn: &Connection, agent_id: &str, days_ago: u32) {
        conn.execute(
            "UPDATE agents SET created_at = datetime('now', ?1) WHERE id = ?2",
            params![format!("-{days_ago} days"), agent_id],
        )
        .unwrap();
    }

    fn age_session(conn: &Connection, session_id: &str, days_ago: u32) {
        conn.execute(
            "UPDATE sessions SET created_at = datetime('now', ?1) WHERE id = ?2",
            params![format!("-{days_ago} days"), session_id],
        )
        .unwrap();
    }

    fn age_task(conn: &Connection, task_id: &str, days_ago: u32) {
        conn.execute(
            "UPDATE tasks SET created_at = datetime('now', ?1), updated_at = datetime('now', ?1) WHERE id = ?2",
            params![format!("-{days_ago} days"), task_id],
        )
        .unwrap();
    }

    #[test]
    fn test_deletes_old_events_and_agents() {
        let conn = setup();
        let session = create_test_session(&conn);

        let e1 = insert_event(&conn, &session.id, None, "prompt_submitted", "{}").unwrap();
        let e2 = insert_event(&conn, &session.id, None, "tool_called", "{}").unwrap();
        age_event(&conn, &e1.id, 60);
        age_event(&conn, &e2.id, 60);

        let a1 = create_agent(
            &conn,
            &session.id,
            None,
            "research",
            "claude-haiku-4-5-20251001",
            "research",
        )
        .unwrap();
        age_agent(&conn, &a1.id, 60);

        let report = run_retention(&conn, 30).unwrap();
        assert_eq!(report.events_deleted, 2);
        assert_eq!(report.agents_deleted, 1);
        assert_eq!(report.tasks_deleted, 0);
        // Session still has recent created_at so not deleted
    }

    #[test]
    fn test_preserves_recent_data() {
        let conn = setup();
        let session = create_test_session(&conn);

        // Recent event (not aged) should be preserved
        insert_event(&conn, &session.id, None, "prompt_submitted", "{}").unwrap();
        create_agent(
            &conn,
            &session.id,
            None,
            "research",
            "claude-haiku-4-5-20251001",
            "research",
        )
        .unwrap();

        let report = run_retention(&conn, 30).unwrap();
        assert_eq!(report.events_deleted, 0);
        assert_eq!(report.agents_deleted, 0);
        assert_eq!(report.tasks_deleted, 0);
        assert_eq!(report.sessions_deleted, 0);
    }

    #[test]
    fn test_preserves_data_in_sessions_with_active_tasks() {
        let conn = setup();
        let session = create_test_session(&conn);

        // Create an active (pending) task in this session
        create_task(
            &conn,
            &session.id,
            "Active task",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();

        // Create old events and agents in the same session
        let e1 = insert_event(&conn, &session.id, None, "prompt_submitted", "{}").unwrap();
        age_event(&conn, &e1.id, 60);

        let a1 = create_agent(
            &conn,
            &session.id,
            None,
            "research",
            "claude-haiku-4-5-20251001",
            "research",
        )
        .unwrap();
        age_agent(&conn, &a1.id, 60);

        // Should NOT delete because session has an active task
        let report = run_retention(&conn, 30).unwrap();
        assert_eq!(report.events_deleted, 0);
        assert_eq!(report.agents_deleted, 0);
        assert_eq!(report.tasks_deleted, 0);
        assert_eq!(report.sessions_deleted, 0);
    }

    #[test]
    fn test_deletes_data_in_sessions_with_only_done_tasks() {
        let conn = setup();
        let session = create_test_session(&conn);

        // Create a done task
        let task = create_task(
            &conn,
            &session.id,
            "Done task",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();
        update_task_status(&conn, &task.id, &TaskStatus::Done, Some("success"), None).unwrap();

        // Create old events and agents
        let e1 = insert_event(&conn, &session.id, None, "prompt_submitted", "{}").unwrap();
        age_event(&conn, &e1.id, 60);

        let a1 = create_agent(
            &conn,
            &session.id,
            None,
            "research",
            "claude-haiku-4-5-20251001",
            "research",
        )
        .unwrap();
        age_agent(&conn, &a1.id, 60);

        // Should delete because all tasks are done
        let report = run_retention(&conn, 30).unwrap();
        assert_eq!(report.events_deleted, 1);
        assert_eq!(report.agents_deleted, 1);
        // Task itself is recent (not aged), so not deleted
        assert_eq!(report.tasks_deleted, 0);
    }

    #[test]
    fn test_mixed_sessions_only_deletes_from_inactive() {
        let conn = setup();

        // Session with active task
        let active_session = create_test_session(&conn);
        create_task(
            &conn,
            &active_session.id,
            "Active",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();
        let e_active =
            insert_event(&conn, &active_session.id, None, "prompt_submitted", "{}").unwrap();
        age_event(&conn, &e_active.id, 60);

        // Session with only done tasks
        let done_session = create_session(&conn, "/tmp/other-project").unwrap();
        let done_task = create_task(
            &conn,
            &done_session.id,
            "Done",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();
        update_task_status(&conn, &done_task.id, &TaskStatus::Done, None, None).unwrap();
        let e_done = insert_event(&conn, &done_session.id, None, "tool_called", "{}").unwrap();
        age_event(&conn, &e_done.id, 60);

        let report = run_retention(&conn, 30).unwrap();
        // Only the event from the done session should be deleted
        assert_eq!(report.events_deleted, 1);
    }

    #[test]
    fn test_stats_rollups_never_deleted() {
        let conn = setup();
        let session = create_test_session(&conn);

        // Insert a rollup
        insert_rollup(
            &conn,
            "session",
            Some(&session.id),
            "2020-01-01T00:00:00",
            "2020-01-02T00:00:00",
            "cost.usd",
            5.0,
        )
        .unwrap();

        // Insert old event to trigger some deletion
        let e1 = insert_event(&conn, &session.id, None, "prompt_submitted", "{}").unwrap();
        age_event(&conn, &e1.id, 60);

        run_retention(&conn, 30).unwrap();

        // Verify rollup still exists
        let rollups = query_rollups(
            &conn,
            "session",
            Some(&session.id),
            "cost.usd",
            "2020-01-01T00:00:00",
        )
        .unwrap();
        assert_eq!(rollups.len(), 1);
    }

    #[test]
    fn test_noop_on_empty_database() {
        let conn = setup();

        let report = run_retention(&conn, 30).unwrap();
        assert_eq!(report.events_deleted, 0);
        assert_eq!(report.agents_deleted, 0);
        assert_eq!(report.tasks_deleted, 0);
        assert_eq!(report.sessions_deleted, 0);
    }

    #[test]
    fn test_deletes_old_tasks_and_sessions() {
        let conn = setup();
        let session = create_test_session(&conn);
        age_session(&conn, &session.id, 60);

        let task = create_task(
            &conn,
            &session.id,
            "Old done task",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();
        update_task_status(&conn, &task.id, &TaskStatus::Done, Some("success"), None).unwrap();
        age_task(&conn, &task.id, 60);

        let report = run_retention(&conn, 30).unwrap();
        assert_eq!(report.tasks_deleted, 1);
        assert_eq!(report.sessions_deleted, 1);
    }

    #[test]
    fn test_deletes_old_task_deps_with_tasks() {
        let conn = setup();
        let session = create_test_session(&conn);
        age_session(&conn, &session.id, 60);

        let t1 = create_task(
            &conn,
            &session.id,
            "Dep A",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();
        let t2 = create_task(
            &conn,
            &session.id,
            "Dep B",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();
        add_task_dep(&conn, &t2.id, &t1.id).unwrap();

        update_task_status(&conn, &t1.id, &TaskStatus::Done, None, None).unwrap();
        update_task_status(&conn, &t2.id, &TaskStatus::Done, None, None).unwrap();
        age_task(&conn, &t1.id, 60);
        age_task(&conn, &t2.id, 60);

        // Should not fail despite FK constraints (task_deps cleaned first)
        let report = run_retention(&conn, 30).unwrap();
        assert_eq!(report.tasks_deleted, 2);
    }

    #[test]
    fn test_respects_ttl_boundary() {
        let conn = setup();
        let session = create_test_session(&conn);

        // Event aged 29 days (within 30-day TTL)
        let e_recent = insert_event(&conn, &session.id, None, "prompt_submitted", "{}").unwrap();
        age_event(&conn, &e_recent.id, 29);

        // Event aged 31 days (outside 30-day TTL)
        let e_old = insert_event(&conn, &session.id, None, "tool_called", "{}").unwrap();
        age_event(&conn, &e_old.id, 31);

        let report = run_retention(&conn, 30).unwrap();
        assert_eq!(report.events_deleted, 1);
    }

    #[test]
    fn test_deletes_old_agents_with_parent_child() {
        let conn = setup();
        let session = create_test_session(&conn);

        let parent =
            create_agent(&conn, &session.id, None, "orchestrator", "model", "mode").unwrap();
        let child = create_agent(
            &conn,
            &session.id,
            Some(&parent.id),
            "research",
            "model",
            "mode",
        )
        .unwrap();

        age_agent(&conn, &parent.id, 60);
        age_agent(&conn, &child.id, 60);

        // Should succeed despite self-referential FK (parent_agent_id nulled first)
        let report = run_retention(&conn, 30).unwrap();
        assert_eq!(report.agents_deleted, 2);
    }

    #[test]
    fn test_deletes_old_tasks_with_parent_child() {
        let conn = setup();
        let session = create_test_session(&conn);

        let parent = create_task(
            &conn,
            &session.id,
            "Parent",
            None,
            None,
            &Priority::High,
            "main",
            "abc",
            "main",
        )
        .unwrap();
        let child = create_task(
            &conn,
            &session.id,
            "Child",
            None,
            Some(&parent.id),
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();

        update_task_status(&conn, &child.id, &TaskStatus::Done, None, None).unwrap();
        update_task_status(&conn, &parent.id, &TaskStatus::Done, None, None).unwrap();
        age_task(&conn, &parent.id, 60);
        age_task(&conn, &child.id, 60);

        // Should succeed despite self-referential FK (parent_id nulled first)
        let report = run_retention(&conn, 30).unwrap();
        assert_eq!(report.tasks_deleted, 2);
    }

    #[test]
    fn test_deletes_old_empty_session() {
        let conn = setup();
        let session = create_test_session(&conn);
        age_session(&conn, &session.id, 60);

        let report = run_retention(&conn, 30).unwrap();
        assert_eq!(report.sessions_deleted, 1);
    }

    #[test]
    fn test_preserves_old_session_with_active_tasks() {
        let conn = setup();
        let session = create_test_session(&conn);
        age_session(&conn, &session.id, 60);

        create_task(
            &conn,
            &session.id,
            "In progress",
            None,
            None,
            &Priority::High,
            "main",
            "abc",
            "main",
        )
        .unwrap();

        let report = run_retention(&conn, 30).unwrap();
        assert_eq!(report.sessions_deleted, 0);
    }

    #[test]
    fn test_preserves_old_session_with_remaining_agents() {
        let conn = setup();
        let session = create_test_session(&conn);
        age_session(&conn, &session.id, 60);

        // Agent is recent — session can't be deleted
        create_agent(&conn, &session.id, None, "research", "model", "mode").unwrap();

        let report = run_retention(&conn, 30).unwrap();
        assert_eq!(report.sessions_deleted, 0);
    }

    #[test]
    fn test_deletes_old_session_after_all_data_cleaned() {
        let conn = setup();
        let session = create_test_session(&conn);
        age_session(&conn, &session.id, 60);

        let e = insert_event(&conn, &session.id, None, "prompt_submitted", "{}").unwrap();
        age_event(&conn, &e.id, 60);
        let a = create_agent(&conn, &session.id, None, "research", "model", "mode").unwrap();
        age_agent(&conn, &a.id, 60);

        let report = run_retention(&conn, 30).unwrap();
        assert_eq!(report.events_deleted, 1);
        assert_eq!(report.agents_deleted, 1);
        assert_eq!(report.sessions_deleted, 1);
    }

    #[test]
    fn test_mixed_sessions_selective_cleanup() {
        let conn = setup();

        // Old session with old done task — eligible for full cleanup
        let old_done = create_session(&conn, "/tmp/old-done").unwrap();
        age_session(&conn, &old_done.id, 60);
        let t = create_task(
            &conn,
            &old_done.id,
            "Done",
            None,
            None,
            &Priority::Low,
            "main",
            "abc",
            "main",
        )
        .unwrap();
        update_task_status(&conn, &t.id, &TaskStatus::Done, None, None).unwrap();
        age_task(&conn, &t.id, 60);

        // Old session with active task — protected
        let old_active = create_session(&conn, "/tmp/old-active").unwrap();
        age_session(&conn, &old_active.id, 60);
        create_task(
            &conn,
            &old_active.id,
            "Pending",
            None,
            None,
            &Priority::Medium,
            "main",
            "abc",
            "main",
        )
        .unwrap();

        // Recent session — not old enough
        create_session(&conn, "/tmp/recent").unwrap();

        let report = run_retention(&conn, 30).unwrap();
        assert_eq!(report.sessions_deleted, 1);
        assert_eq!(report.tasks_deleted, 1);
    }
}
