use sqlx::SqlitePool;

use crate::db::test_pool;

use super::decisions::{
    build_config_change_action, build_mode_create_action, build_mode_edit_action,
    build_model_change_action, build_prompt_edit_action, generate_candidates, DecisionContext,
};
use super::learning::{build_learning_context, extract_conventions, Correction, CorrectionType};
use super::lifecycle::{LifecycleManager, StubConfigOps, StubModeOps};
use super::prompt::build_user_message;
use super::runtime::{StubModelInvoker, UxAgentRuntime};
use super::store::RecommendationStore;
use super::triggers::{evaluate_triggers, EventSummary, TriggerReason};
use super::types::{
    ModeChanges, Recommendation, RecommendationAction, RecommendationStatus, UxAgentState,
};

async fn test_db() -> SqlitePool {
    test_pool().await
}

// ---------------------------------------------------------------------------
// 1. Types tests
// ---------------------------------------------------------------------------

#[test]
fn recommendation_action_serialization_roundtrip() {
    let actions: Vec<RecommendationAction> = vec![
        RecommendationAction::ModelChange {
            role: "planner".into(),
            from_model: "cheap".into(),
            to_model: "smart".into(),
        },
        RecommendationAction::PromptEdit {
            mode_name: "strict".into(),
            old_fragment: "be terse".into(),
            new_fragment: "be verbose".into(),
        },
        RecommendationAction::ModeCreate {
            name: "db-mode".into(),
            description: "Database work".into(),
            system_prompt: "You are a DB expert".into(),
            default_model: Some("gpt-4".into()),
            allowed_tools: vec!["sql".into(), "migrate".into()],
        },
        RecommendationAction::ModeEdit {
            mode_name: "strict".into(),
            changes: ModeChanges {
                description: Some("updated".into()),
                system_prompt: None,
                default_model: None,
                allowed_tools: Some(vec!["code_edit".into()]),
            },
        },
        RecommendationAction::ConfigChange {
            key: "timeout".into(),
            old_value: "30".into(),
            new_value: "60".into(),
        },
    ];

    for action in &actions {
        let json = serde_json::to_string(action).unwrap();
        let decoded: RecommendationAction = serde_json::from_str(&json).unwrap();
        assert_eq!(&decoded, action, "roundtrip failed for {json}");
    }
}

#[test]
fn recommendation_status_serialization() {
    for (status, expected) in [
        (RecommendationStatus::Pending, "Pending"),
        (RecommendationStatus::Applied, "Applied"),
        (RecommendationStatus::Dismissed, "Dismissed"),
        (RecommendationStatus::Reverted, "Reverted"),
    ] {
        assert_eq!(status.to_string(), expected);
        let parsed: RecommendationStatus = expected.parse().unwrap();
        assert_eq!(parsed, status);
    }
}

#[test]
fn recommendation_status_case_insensitive_parse() {
    assert_eq!(
        "pending".parse::<RecommendationStatus>().unwrap(),
        RecommendationStatus::Pending,
    );
    assert_eq!(
        "applied".parse::<RecommendationStatus>().unwrap(),
        RecommendationStatus::Applied,
    );
}

#[test]
fn recommendation_status_unknown_errors() {
    assert!("unknown".parse::<RecommendationStatus>().is_err());
}

#[test]
fn mode_changes_partial_fields() {
    let changes = ModeChanges {
        description: Some("new desc".into()),
        system_prompt: None,
        default_model: None,
        allowed_tools: None,
    };
    let json = serde_json::to_string(&changes).unwrap();
    let decoded: ModeChanges = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.description, Some("new desc".into()));
    assert_eq!(decoded.system_prompt, None);
    assert_eq!(decoded.default_model, None);
    assert_eq!(decoded.allowed_tools, None);
}

#[test]
fn mode_changes_all_fields_some() {
    let changes = ModeChanges {
        description: Some("d".into()),
        system_prompt: Some("sp".into()),
        default_model: Some("m".into()),
        allowed_tools: Some(vec!["t1".into()]),
    };
    let json = serde_json::to_string(&changes).unwrap();
    let decoded: ModeChanges = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, changes);
}

#[test]
fn ux_agent_state_default() {
    let state = UxAgentState::default();
    assert_eq!(state.last_event_id, None);
    assert_eq!(state.last_event_at, None);
    assert_eq!(state.last_run_at, None);
}

#[test]
fn recommendation_equality() {
    let a = Recommendation {
        id: 1,
        trigger_pattern: "test".into(),
        recommendation: "do something".into(),
        action: RecommendationAction::ConfigChange {
            key: "k".into(),
            old_value: "a".into(),
            new_value: "b".into(),
        },
        status: RecommendationStatus::Pending,
    };
    let b = a.clone();
    assert_eq!(a, b);
}

// ---------------------------------------------------------------------------
// 2. Store tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn store_insert_and_get_each_action_type() {
    let pool = test_db().await;
    let store = RecommendationStore::new(pool.clone());

    let actions: Vec<RecommendationAction> = vec![
        RecommendationAction::ModelChange {
            role: "planner".into(),
            from_model: "a".into(),
            to_model: "b".into(),
        },
        RecommendationAction::PromptEdit {
            mode_name: "strict".into(),
            old_fragment: "old".into(),
            new_fragment: "new".into(),
        },
        RecommendationAction::ModeCreate {
            name: "m".into(),
            description: "d".into(),
            system_prompt: "sp".into(),
            default_model: None,
            allowed_tools: vec![],
        },
        RecommendationAction::ModeEdit {
            mode_name: "strict".into(),
            changes: ModeChanges {
                description: Some("d".into()),
                system_prompt: None,
                default_model: None,
                allowed_tools: None,
            },
        },
        RecommendationAction::ConfigChange {
            key: "k".into(),
            old_value: "a".into(),
            new_value: "b".into(),
        },
    ];

    for action in actions {
        let rec = Recommendation {
            id: 0,
            trigger_pattern: "tp".into(),
            recommendation: "r".into(),
            action: action.clone(),
            status: RecommendationStatus::Pending,
        };
        let id = store.insert(&rec).await.unwrap();
        let loaded = store.get(id).await.unwrap().unwrap();
        assert_eq!(loaded.action, action);
        assert_eq!(loaded.status, RecommendationStatus::Pending);
    }
}

#[tokio::test]
async fn store_list_pending_filters_correctly() {
    let pool = test_db().await;
    let store = RecommendationStore::new(pool.clone());

    let make = |status: RecommendationStatus| Recommendation {
        id: 0,
        trigger_pattern: "tp".into(),
        recommendation: "r".into(),
        action: RecommendationAction::ConfigChange {
            key: "k".into(),
            old_value: "a".into(),
            new_value: "b".into(),
        },
        status,
    };

    let id1 = store
        .insert(&make(RecommendationStatus::Pending))
        .await
        .unwrap();
    let id2 = store
        .insert(&make(RecommendationStatus::Pending))
        .await
        .unwrap();
    let id3 = store
        .insert(&make(RecommendationStatus::Pending))
        .await
        .unwrap();

    store
        .update_status(id3, RecommendationStatus::Applied)
        .await
        .unwrap();

    let pending = store.list_pending().await.unwrap();
    assert_eq!(pending.len(), 2);
    let ids: Vec<u64> = pending.iter().map(|r| r.id).collect();
    assert!(ids.contains(&id1));
    assert!(ids.contains(&id2));
}

#[tokio::test]
async fn store_cursor_default() {
    let pool = test_db().await;
    let store = RecommendationStore::new(pool.clone());
    let state = store.get_cursor().await.unwrap();
    assert_eq!(state, UxAgentState::default());
}

#[tokio::test]
async fn store_cursor_roundtrip() {
    let pool = test_db().await;
    let store = RecommendationStore::new(pool.clone());

    let state = UxAgentState {
        last_event_id: Some("evt-123".into()),
        last_event_at: Some("2026-02-22T10:00:00Z".into()),
        last_run_at: Some("2026-02-22T10:05:00Z".into()),
    };
    store.set_cursor(&state).await.unwrap();
    let loaded = store.get_cursor().await.unwrap();
    assert_eq!(loaded, state);

    // Update cursor again
    let state2 = UxAgentState {
        last_event_id: Some("evt-456".into()),
        last_event_at: Some("2026-02-22T11:00:00Z".into()),
        last_run_at: Some("2026-02-22T11:05:00Z".into()),
    };
    store.set_cursor(&state2).await.unwrap();
    let loaded2 = store.get_cursor().await.unwrap();
    assert_eq!(loaded2, state2);
}

#[tokio::test]
async fn store_version_tracking() {
    let pool = test_db().await;
    let store = RecommendationStore::new(pool.clone());

    let rec = Recommendation {
        id: 0,
        trigger_pattern: "tp".into(),
        recommendation: "r".into(),
        action: RecommendationAction::ConfigChange {
            key: "k".into(),
            old_value: "a".into(),
            new_value: "b".into(),
        },
        status: RecommendationStatus::Pending,
    };
    let rec_id = store.insert(&rec).await.unwrap();

    store
        .insert_version(rec_id, 1, r#"{"key":"k","value":"a"}"#)
        .await
        .unwrap();

    let versions = store.get_versions(rec_id).await.unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].version, 1);
    assert_eq!(versions[0].recommendation_id, rec_id);
    assert!(versions[0].reverted_at.is_none());

    store.mark_version_reverted(versions[0].id).await.unwrap();
    let versions2 = store.get_versions(rec_id).await.unwrap();
    assert!(versions2[0].reverted_at.is_some());
}

#[tokio::test]
async fn store_dismissed_patterns() {
    let pool = test_db().await;
    let store = RecommendationStore::new(pool.clone());

    let rec = Recommendation {
        id: 0,
        trigger_pattern: "cost_spike".into(),
        recommendation: "r".into(),
        action: RecommendationAction::ConfigChange {
            key: "k".into(),
            old_value: "a".into(),
            new_value: "b".into(),
        },
        status: RecommendationStatus::Pending,
    };
    let id = store.insert(&rec).await.unwrap();
    store
        .update_status(id, RecommendationStatus::Dismissed)
        .await
        .unwrap();

    let patterns = store.get_dismissed_patterns().await.unwrap();
    assert_eq!(patterns, vec!["cost_spike"]);
}

// ---------------------------------------------------------------------------
// 3. Trigger tests
// ---------------------------------------------------------------------------

#[test]
fn no_trigger_without_new_events() {
    let summary = EventSummary {
        rejection_count: 10,
        is_new_session: true,
        has_new_events: false,
        ..Default::default()
    };
    assert!(evaluate_triggers(&summary).is_empty());
}

#[test]
fn rejection_trigger() {
    let summary = EventSummary {
        rejection_count: 3,
        has_new_events: true,
        ..Default::default()
    };
    let triggers = evaluate_triggers(&summary);
    assert_eq!(triggers.len(), 1);
    assert!(matches!(
        triggers[0],
        TriggerReason::RejectionsAccumulated { count: 3 }
    ));
}

#[test]
fn rejection_below_threshold() {
    let summary = EventSummary {
        rejection_count: 2,
        has_new_events: true,
        ..Default::default()
    };
    assert!(evaluate_triggers(&summary).is_empty());
}

#[test]
fn new_session_trigger() {
    let summary = EventSummary {
        is_new_session: true,
        has_new_events: true,
        ..Default::default()
    };
    let triggers = evaluate_triggers(&summary);
    assert_eq!(triggers, vec![TriggerReason::NewSession]);
}

#[test]
fn cost_spike_trigger() {
    let summary = EventSummary {
        recent_cost_usd: 10.0,
        baseline_cost_usd: 3.0,
        has_new_events: true,
        ..Default::default()
    };
    let triggers = evaluate_triggers(&summary);
    assert_eq!(triggers.len(), 1);
    assert!(matches!(triggers[0], TriggerReason::CostSpike { .. }));
}

#[test]
fn cost_spike_zero_baseline() {
    let summary = EventSummary {
        recent_cost_usd: 100.0,
        baseline_cost_usd: 0.0,
        has_new_events: true,
        ..Default::default()
    };
    assert!(evaluate_triggers(&summary).is_empty());
}

#[test]
fn failure_pattern_trigger() {
    let summary = EventSummary {
        tool_failures: vec![("shell".to_string(), 5)],
        has_new_events: true,
        ..Default::default()
    };
    let triggers = evaluate_triggers(&summary);
    assert_eq!(triggers.len(), 1);
    assert!(matches!(
        &triggers[0],
        TriggerReason::FailurePattern { tool, failure_count: 5 } if tool == "shell"
    ));
}

#[test]
fn override_pattern_trigger() {
    let summary = EventSummary {
        mode_overrides: vec![("General->Implement".to_string(), 4)],
        has_new_events: true,
        ..Default::default()
    };
    let triggers = evaluate_triggers(&summary);
    assert_eq!(triggers.len(), 1);
    assert!(matches!(
        &triggers[0],
        TriggerReason::OverridePattern { override_type, count: 4 } if override_type == "General->Implement"
    ));
}

#[test]
fn multiple_triggers_simultaneously() {
    let summary = EventSummary {
        rejection_count: 5,
        is_new_session: true,
        recent_cost_usd: 10.0,
        baseline_cost_usd: 3.0,
        tool_failures: vec![("bash".to_string(), 3)],
        mode_overrides: vec![("auto_approve".to_string(), 3)],
        has_new_events: true,
    };
    let triggers = evaluate_triggers(&summary);
    assert_eq!(triggers.len(), 5);
}

// ---------------------------------------------------------------------------
// 4. Decision generation tests
// ---------------------------------------------------------------------------

fn test_decision_context() -> DecisionContext {
    DecisionContext {
        available_modes: vec!["default".into(), "strict".into()],
        current_models: vec![
            ("planner".into(), "gpt-4".into()),
            ("coder".into(), "gpt-3.5".into()),
        ],
        dismissed_patterns: vec![],
        recent_applied: vec![],
    }
}

#[test]
fn generate_candidates_for_rejections() {
    let triggers = vec![TriggerReason::RejectionsAccumulated { count: 5 }];
    let candidates = generate_candidates(
        &triggers,
        &EventSummary::default(),
        &test_decision_context(),
    );
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].suggested_action_type, "mode_edit");
}

#[test]
fn generate_candidates_filters_dismissed() {
    let triggers = vec![
        TriggerReason::RejectionsAccumulated { count: 5 },
        TriggerReason::NewSession,
    ];
    let mut ctx = test_decision_context();
    ctx.dismissed_patterns = vec!["rejections_accumulated".into()];

    let candidates = generate_candidates(&triggers, &EventSummary::default(), &ctx);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].trigger, "new_session");
}

#[test]
fn action_builder_mode_create() {
    let action = build_mode_create_action(
        "db",
        "DB work",
        "You are a DB expert",
        Some("gpt-4"),
        &["sql"],
    );
    assert!(matches!(
        action,
        RecommendationAction::ModeCreate { ref name, ref default_model, .. }
        if name == "db" && default_model.as_deref() == Some("gpt-4")
    ));
}

#[test]
fn action_builder_mode_edit() {
    let changes = ModeChanges {
        description: Some("updated".into()),
        system_prompt: None,
        default_model: None,
        allowed_tools: None,
    };
    let action = build_mode_edit_action("strict", changes.clone());
    assert_eq!(
        action,
        RecommendationAction::ModeEdit {
            mode_name: "strict".into(),
            changes,
        }
    );
}

#[test]
fn action_builder_model_change() {
    let action = build_model_change_action("coder", "gpt-4", "gpt-3.5");
    assert_eq!(
        action,
        RecommendationAction::ModelChange {
            role: "coder".into(),
            from_model: "gpt-4".into(),
            to_model: "gpt-3.5".into(),
        }
    );
}

#[test]
fn action_builder_config_change() {
    let action = build_config_change_action("timeout", "30", "60");
    assert_eq!(
        action,
        RecommendationAction::ConfigChange {
            key: "timeout".into(),
            old_value: "30".into(),
            new_value: "60".into(),
        }
    );
}

#[test]
fn action_builder_prompt_edit() {
    let action = build_prompt_edit_action("strict", "old", "new");
    assert_eq!(
        action,
        RecommendationAction::PromptEdit {
            mode_name: "strict".into(),
            old_fragment: "old".into(),
            new_fragment: "new".into(),
        }
    );
}

// ---------------------------------------------------------------------------
// 5. Lifecycle tests
// ---------------------------------------------------------------------------

async fn insert_pending(pool: &SqlitePool, action: RecommendationAction) -> u64 {
    let store = RecommendationStore::new(pool.clone());
    store
        .insert(&Recommendation {
            id: 0,
            trigger_pattern: "test-pattern".into(),
            recommendation: "test rec".into(),
            action,
            status: RecommendationStatus::Pending,
        })
        .await
        .unwrap()
}

fn model_change_action() -> RecommendationAction {
    RecommendationAction::ModelChange {
        role: "planner".into(),
        from_model: "cheap".into(),
        to_model: "smart".into(),
    }
}

#[tokio::test]
async fn apply_pending_recommendation() {
    let pool = test_db().await;
    let id = insert_pending(&pool, model_change_action()).await;

    LifecycleManager::apply(&pool, id, &StubModeOps, &StubConfigOps)
        .await
        .unwrap();

    let store = RecommendationStore::new(pool.clone());
    let rec = store.get(id).await.unwrap().unwrap();
    assert_eq!(rec.status, RecommendationStatus::Applied);

    let versions = store.get_versions(id).await.unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].version, 1);
}

#[tokio::test]
async fn apply_non_pending_fails() {
    let pool = test_db().await;
    let id = insert_pending(&pool, model_change_action()).await;

    LifecycleManager::apply(&pool, id, &StubModeOps, &StubConfigOps)
        .await
        .unwrap();

    let err = LifecycleManager::apply(&pool, id, &StubModeOps, &StubConfigOps)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("expected Pending"));
}

#[tokio::test]
async fn dismiss_recommendation() {
    let pool = test_db().await;
    let id = insert_pending(&pool, model_change_action()).await;

    LifecycleManager::dismiss(&pool, id).await.unwrap();

    let store = RecommendationStore::new(pool.clone());
    let rec = store.get(id).await.unwrap().unwrap();
    assert_eq!(rec.status, RecommendationStatus::Dismissed);
}

#[tokio::test]
async fn revert_applied_recommendation() {
    let pool = test_db().await;
    let id = insert_pending(&pool, model_change_action()).await;

    LifecycleManager::apply(&pool, id, &StubModeOps, &StubConfigOps)
        .await
        .unwrap();
    LifecycleManager::revert(&pool, id, &StubModeOps, &StubConfigOps)
        .await
        .unwrap();

    let store = RecommendationStore::new(pool.clone());
    let rec = store.get(id).await.unwrap().unwrap();
    assert_eq!(rec.status, RecommendationStatus::Reverted);

    let versions = store.get_versions(id).await.unwrap();
    assert!(versions[0].reverted_at.is_some());
}

#[tokio::test]
async fn revert_non_applied_fails() {
    let pool = test_db().await;
    let id = insert_pending(&pool, model_change_action()).await;

    let err = LifecycleManager::revert(&pool, id, &StubModeOps, &StubConfigOps)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("expected Applied"));
}

#[tokio::test]
async fn dismissed_patterns_returned() {
    let pool = test_db().await;
    let id = insert_pending(&pool, model_change_action()).await;

    LifecycleManager::dismiss(&pool, id).await.unwrap();

    let store = RecommendationStore::new(pool.clone());
    let patterns = store.get_dismissed_patterns().await.unwrap();
    assert_eq!(patterns, vec!["test-pattern"]);
}

// ---------------------------------------------------------------------------
// 6. Learning tests
// ---------------------------------------------------------------------------

fn make_correction(
    ct: CorrectionType,
    original: Option<&str>,
    corrected: Option<&str>,
) -> Correction {
    Correction {
        id: 0,
        session_id: Some("test-session".into()),
        correction_type: ct,
        original_value: original.map(|s| s.to_string()),
        corrected_value: corrected.map(|s| s.to_string()),
        context: None,
        created_at: String::new(),
        incorporated: false,
    }
}

#[tokio::test]
async fn insert_and_retrieve_corrections() {
    let pool = test_db().await;
    let store = RecommendationStore::new(pool.clone());

    store
        .insert_correction(&make_correction(
            CorrectionType::ModeOverride,
            Some("A"),
            Some("B"),
        ))
        .await
        .unwrap();
    store
        .insert_correction(&make_correction(
            CorrectionType::PlanRejection,
            None,
            Some("verbose"),
        ))
        .await
        .unwrap();
    store
        .insert_correction(&make_correction(
            CorrectionType::AgentSteering,
            None,
            Some("use tests"),
        ))
        .await
        .unwrap();

    let all = store.get_unincorporated_corrections().await.unwrap();
    assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn mark_corrections_incorporated() {
    let pool = test_db().await;
    let store = RecommendationStore::new(pool.clone());

    let id1 = store
        .insert_correction(&make_correction(
            CorrectionType::ModeOverride,
            Some("A"),
            Some("B"),
        ))
        .await
        .unwrap();
    let id2 = store
        .insert_correction(&make_correction(
            CorrectionType::DiffRejection,
            None,
            Some("bad"),
        ))
        .await
        .unwrap();
    let id3 = store
        .insert_correction(&make_correction(
            CorrectionType::AgentSteering,
            None,
            Some("tests"),
        ))
        .await
        .unwrap();

    store
        .mark_corrections_incorporated(&[id1, id2])
        .await
        .unwrap();

    let remaining = store.get_unincorporated_corrections().await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, id3);
}

#[test]
fn extract_conventions_from_overrides() {
    let corrections: Vec<Correction> = (0..4)
        .map(|i| Correction {
            id: i,
            session_id: None,
            correction_type: CorrectionType::ModeOverride,
            original_value: Some("General".into()),
            corrected_value: Some("Implement".into()),
            context: None,
            created_at: String::new(),
            incorporated: false,
        })
        .collect();

    let conventions = extract_conventions(&corrections);
    assert_eq!(conventions.len(), 1);
    assert!(conventions[0].convention.contains("Implement"));
    assert!(conventions[0].convention.contains("General"));
    assert_eq!(conventions[0].correction_ids.len(), 4);
    assert_eq!(conventions[0].target_mode.as_deref(), Some("Implement"));
}

#[test]
fn extract_conventions_insufficient_data() {
    let corrections: Vec<Correction> = vec![Correction {
        id: 1,
        session_id: None,
        correction_type: CorrectionType::ModeOverride,
        original_value: Some("A".into()),
        corrected_value: Some("B".into()),
        context: None,
        created_at: String::new(),
        incorporated: false,
    }];

    let conventions = extract_conventions(&corrections);
    assert!(conventions.is_empty());
}

#[tokio::test]
async fn build_learning_context_formatting() {
    let pool = test_db().await;
    let store = RecommendationStore::new(pool.clone());

    for _ in 0..3 {
        store
            .insert_correction(&make_correction(
                CorrectionType::ModeOverride,
                Some("General"),
                Some("Implement"),
            ))
            .await
            .unwrap();
    }

    let context = build_learning_context(&store).await.unwrap();
    assert!(!context.is_empty());
    assert!(context.contains("Corrections since last incorporation"));
    assert!(context.contains("mode_override"));
}

#[tokio::test]
async fn build_learning_context_empty_when_no_data() {
    let pool = test_db().await;
    let store = RecommendationStore::new(pool.clone());
    let context = build_learning_context(&store).await.unwrap();
    assert!(context.is_empty());
}

#[tokio::test]
async fn convention_lifecycle() {
    let pool = test_db().await;
    let store = RecommendationStore::new(pool.clone());

    let id = store
        .insert_convention("always use migrations", &[1, 2, 3], Some("implement"))
        .await
        .unwrap();
    assert!(id > 0);

    let proposed = store.get_proposed_conventions().await.unwrap();
    assert_eq!(proposed.len(), 1);
    assert_eq!(proposed[0].convention, "always use migrations");
    assert_eq!(proposed[0].source_corrections, vec![1, 2, 3]);
    assert_eq!(proposed[0].target_mode.as_deref(), Some("implement"));
    assert_eq!(proposed[0].status, "proposed");

    store.update_convention_status(id, "applied").await.unwrap();
    let proposed_after = store.get_proposed_conventions().await.unwrap();
    assert!(proposed_after.is_empty());
}

// ---------------------------------------------------------------------------
// 7. Runtime integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runtime_run_with_stub_model() {
    let pool = test_db().await;
    let rt = UxAgentRuntime::new("cheap-model".into(), Box::new(StubModelInvoker));
    let triggers = vec![TriggerReason::RejectionsAccumulated { count: 3 }];
    let summary = EventSummary {
        rejection_count: 3,
        has_new_events: true,
        ..Default::default()
    };

    let recs = rt
        .run(&pool, &triggers, &summary, "{}", "[]")
        .await
        .unwrap();
    assert!(recs.is_empty());

    // Cursor should be updated even with no recommendations
    let store = RecommendationStore::new(pool.clone());
    let cursor = store.get_cursor().await.unwrap();
    assert!(cursor.last_run_at.is_some());
}

struct MockModelInvoker {
    response: String,
}

impl super::runtime::ModelInvoker for MockModelInvoker {
    fn invoke(
        &self,
        _system_prompt: &str,
        _user_message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.response.clone())
    }
}

#[tokio::test]
async fn runtime_persists_recommendations() {
    let pool = test_db().await;
    let response = serde_json::json!({
        "recommendations": [{
            "trigger_pattern": "3+ rejections on schema edits",
            "recommendation": "Switch planner model",
            "action": {
                "type": "ModelChange",
                "role": "planner",
                "from_model": "cheap",
                "to_model": "smart"
            }
        }]
    });
    let invoker = MockModelInvoker {
        response: serde_json::to_string(&response).unwrap(),
    };
    let rt = UxAgentRuntime::new("test-model".into(), Box::new(invoker));
    let triggers = vec![TriggerReason::RejectionsAccumulated { count: 3 }];
    let summary = EventSummary {
        rejection_count: 3,
        has_new_events: true,
        ..Default::default()
    };

    let recs = rt
        .run(&pool, &triggers, &summary, "{}", "[]")
        .await
        .unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].status, RecommendationStatus::Pending);

    let store = RecommendationStore::new(pool.clone());
    let pending = store.list_pending().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, recs[0].id);
}

#[test]
fn prompt_assembly_contains_trigger_info() {
    let triggers = vec![
        TriggerReason::RejectionsAccumulated { count: 5 },
        TriggerReason::CostSpike {
            current_rate_usd: 10.0,
            baseline_rate_usd: 3.0,
        },
    ];
    let summary = EventSummary {
        rejection_count: 5,
        recent_cost_usd: 10.0,
        baseline_cost_usd: 3.0,
        has_new_events: true,
        tool_failures: vec![("bash".into(), 4)],
        ..Default::default()
    };

    let msg = build_user_message(&triggers, &summary, "{}", "[]", &[]);
    assert!(msg.contains("5 rejections accumulated"));
    assert!(msg.contains("Cost spike"));
    assert!(msg.contains("Rejections: 5"));
    assert!(msg.contains("bash: 4 failures"));
}

// ---------------------------------------------------------------------------
// 8. End-to-end pipeline: triggers -> runtime -> lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_pipeline_trigger_to_apply_to_revert() {
    let pool = test_db().await;

    // 1. Evaluate triggers
    let summary = EventSummary {
        rejection_count: 5,
        has_new_events: true,
        ..Default::default()
    };
    let triggers = evaluate_triggers(&summary);
    assert!(!triggers.is_empty());

    // 2. Run the UX agent runtime with a mock that produces a recommendation
    let response = serde_json::json!({
        "recommendations": [{
            "trigger_pattern": "5+ rejections",
            "recommendation": "Switch planner model for schema tasks",
            "action": {
                "type": "ModelChange",
                "role": "planner",
                "from_model": "cheap",
                "to_model": "smart"
            }
        }]
    });
    let invoker = MockModelInvoker {
        response: serde_json::to_string(&response).unwrap(),
    };
    let rt = UxAgentRuntime::new("ux-model".into(), Box::new(invoker));
    let recs = rt
        .run(&pool, &triggers, &summary, "{}", "[]")
        .await
        .unwrap();
    assert_eq!(recs.len(), 1);
    let rec_id = recs[0].id;

    // 3. Apply the recommendation
    LifecycleManager::apply(&pool, rec_id, &StubModeOps, &StubConfigOps)
        .await
        .unwrap();
    let store = RecommendationStore::new(pool.clone());
    let rec = store.get(rec_id).await.unwrap().unwrap();
    assert_eq!(rec.status, RecommendationStatus::Applied);

    // 4. Revert the recommendation
    LifecycleManager::revert(&pool, rec_id, &StubModeOps, &StubConfigOps)
        .await
        .unwrap();
    let rec = store.get(rec_id).await.unwrap().unwrap();
    assert_eq!(rec.status, RecommendationStatus::Reverted);
}

#[tokio::test]
async fn full_pipeline_trigger_to_dismiss_affects_future_candidates() {
    let pool = test_db().await;

    // 1. Create and dismiss a recommendation
    let response = serde_json::json!({
        "recommendations": [{
            "trigger_pattern": "rejections_accumulated:5",
            "recommendation": "Adjust mode",
            "action": {
                "type": "ModeEdit",
                "mode_name": "default",
                "changes": {
                    "description": "adjusted",
                    "system_prompt": null,
                    "default_model": null,
                    "allowed_tools": null
                }
            }
        }]
    });
    let invoker = MockModelInvoker {
        response: serde_json::to_string(&response).unwrap(),
    };
    let rt = UxAgentRuntime::new("ux-model".into(), Box::new(invoker));
    let summary = EventSummary {
        rejection_count: 5,
        has_new_events: true,
        ..Default::default()
    };
    let triggers = evaluate_triggers(&summary);
    let recs = rt
        .run(&pool, &triggers, &summary, "{}", "[]")
        .await
        .unwrap();
    let rec_id = recs[0].id;

    LifecycleManager::dismiss(&pool, rec_id).await.unwrap();

    // 2. Dismissed patterns now include the trigger pattern
    let store = RecommendationStore::new(pool.clone());
    let dismissed = store.get_dismissed_patterns().await.unwrap();
    assert!(dismissed.contains(&"rejections_accumulated:5".to_string()));

    // 3. Future decision generation should filter it out
    let ctx = DecisionContext {
        available_modes: vec!["default".into()],
        current_models: vec![],
        dismissed_patterns: dismissed,
        recent_applied: vec![],
    };
    let candidates = generate_candidates(&triggers, &summary, &ctx);
    // The rejections_accumulated candidate should be filtered
    assert!(
        candidates
            .iter()
            .all(|c| !c.trigger.contains("rejections_accumulated")),
        "dismissed pattern should be filtered from candidates"
    );
}

#[tokio::test]
async fn learning_corrections_feed_into_context() {
    let pool = test_db().await;
    let store = RecommendationStore::new(pool.clone());

    // Insert corrections that form a pattern
    for _ in 0..3 {
        store
            .insert_correction(&make_correction(
                CorrectionType::ModeOverride,
                Some("General"),
                Some("Implement"),
            ))
            .await
            .unwrap();
    }
    store
        .insert_correction(&make_correction(
            CorrectionType::PlanRejection,
            None,
            Some("too verbose"),
        ))
        .await
        .unwrap();

    // Build learning context
    let context = build_learning_context(&store).await.unwrap();
    assert!(context.contains("mode_override"));
    assert!(context.contains("Corrections since last incorporation"));

    // Extract conventions
    let corrections = store.get_unincorporated_corrections().await.unwrap();
    let conventions = extract_conventions(&corrections);
    assert!(!conventions.is_empty());
    assert!(conventions
        .iter()
        .any(|c| c.convention.contains("Implement")));

    // Mark corrections as incorporated
    let ids: Vec<u64> = corrections.iter().map(|c| c.id).collect();
    store.mark_corrections_incorporated(&ids).await.unwrap();
    let remaining = store.get_unincorporated_corrections().await.unwrap();
    assert!(remaining.is_empty());
}
