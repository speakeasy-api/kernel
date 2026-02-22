use std::cmp::Ordering;

use uuid::Uuid;

use super::orchestrator::SpawnRequest;
use super::types::{AgentRole, BranchConfig, CompactedContextRef, TokenMetrics};

#[derive(Debug, Clone)]
pub struct BranchResult {
    pub agent_id: Uuid,
    pub model: String,
    pub summary: String,
    pub token_usage: TokenMetrics,
    pub success: bool,
    pub rank: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct BranchSession {
    pub id: Uuid,
    pub config: BranchConfig,
    pub prompt: String,
    pub branch_agent_ids: Vec<Uuid>,
    pub results: Vec<BranchResult>,
    pub status: BranchStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchStatus {
    /// Branches are being set up.
    Preparing,
    /// All branch agents have been spawned and are running.
    Running,
    /// All branches completed, results available.
    Completed,
    /// Some branches failed.
    PartiallyCompleted,
    /// All branches failed.
    Failed,
}

pub struct BranchManager;

impl BranchManager {
    /// Create spawn requests for all branches.
    /// Each branch gets the same prompt/context but potentially different models.
    pub fn create_branch_requests(
        config: &BranchConfig,
        _prompt: &str,
        role: AgentRole,
        mode: &str,
        context: CompactedContextRef,
        default_model: &str,
    ) -> Vec<SpawnRequest> {
        (0..config.parallel_count)
            .map(|i| {
                let model = if config.models.is_empty() {
                    default_model.to_string()
                } else {
                    config.models[i % config.models.len()].clone()
                };

                SpawnRequest {
                    role: role.clone(),
                    model: Some(model),
                    mode: mode.to_string(),
                    context: CompactedContextRef {
                        summary: format!(
                            "{}\n\nBranch {}/{}: Explore independently.",
                            context.summary,
                            i + 1,
                            config.parallel_count
                        ),
                        token_count: context.token_count,
                    },
                    allowed_tools: super::routing::default_tools_for_role(&role),
                }
            })
            .collect()
    }

    /// Create a new branch session from config and prompt.
    pub fn create_session(config: BranchConfig, prompt: String) -> BranchSession {
        assert!(
            config.parallel_count >= 2,
            "branching mode requires parallel_count >= 2"
        );

        BranchSession {
            id: Uuid::new_v4(),
            config,
            prompt,
            branch_agent_ids: Vec::new(),
            results: Vec::new(),
            status: BranchStatus::Preparing,
        }
    }

    /// Record a branch agent spawn in the session.
    pub fn record_spawn(session: &mut BranchSession, agent_id: Uuid) {
        session.branch_agent_ids.push(agent_id);
        if session.status == BranchStatus::Preparing {
            session.status = BranchStatus::Running;
        }
    }

    /// Record a branch completion result in the session.
    pub fn record_result(session: &mut BranchSession, result: BranchResult) {
        if let Some(existing) = session
            .results
            .iter_mut()
            .find(|existing| existing.agent_id == result.agent_id)
        {
            *existing = result;
        } else {
            session.results.push(result);
        }
        Self::update_status(session);
    }

    /// Check whether all branches have reported results.
    pub fn is_complete(session: &BranchSession) -> bool {
        session.results.len() == session.config.parallel_count
    }

    /// Update session status from result state.
    pub fn update_status(session: &mut BranchSession) {
        if !Self::is_complete(session) {
            session.status = BranchStatus::Running;
            return;
        }

        let success_count = session
            .results
            .iter()
            .filter(|result| result.success)
            .count();
        let failure_count = session.results.len() - success_count;

        session.status = if success_count == session.results.len() {
            BranchStatus::Completed
        } else if failure_count == session.results.len() {
            BranchStatus::Failed
        } else {
            BranchStatus::PartiallyCompleted
        };
    }

    /// Return ranked results (rank if available, otherwise cost efficiency).
    pub fn ranked_results(session: &BranchSession) -> Vec<&BranchResult> {
        let mut ordered: Vec<&BranchResult> = session.results.iter().collect();

        if session.config.auto_rank && session.results.iter().any(|result| result.rank.is_some()) {
            ordered.sort_by(|a, b| match (a.rank, b.rank) {
                (Some(left), Some(right)) => left.cmp(&right),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => a.token_usage.cost_usd.total_cmp(&b.token_usage.cost_usd),
            });
        } else {
            ordered.sort_by(|a, b| {
                a.token_usage
                    .cost_usd
                    .total_cmp(&b.token_usage.cost_usd)
                    .then_with(|| {
                        (a.token_usage.input + a.token_usage.output)
                            .cmp(&(b.token_usage.input + b.token_usage.output))
                    })
            });
        }

        ordered
    }

    /// Sum token usage across all branch results.
    pub fn total_tokens(session: &BranchSession) -> TokenMetrics {
        session
            .results
            .iter()
            .fold(TokenMetrics::default(), |acc, result| {
                acc + result.token_usage.clone()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_context() -> CompactedContextRef {
        CompactedContextRef {
            summary: "Compacted context summary".to_string(),
            token_count: 123,
        }
    }

    fn sample_tokens(input: u64, output: u64, cost_usd: f64) -> TokenMetrics {
        TokenMetrics {
            input,
            output,
            cost_usd,
        }
    }

    #[test]
    fn create_branch_requests_cycles_models() {
        let config = BranchConfig {
            parallel_count: 5,
            models: vec!["model-a".to_string(), "model-b".to_string()],
            auto_rank: false,
        };

        let requests = BranchManager::create_branch_requests(
            &config,
            "Solve this task",
            AgentRole::Research,
            "research",
            sample_context(),
            "default-model",
        );

        assert_eq!(requests.len(), 5);
        assert_eq!(requests[0].model.as_deref(), Some("model-a"));
        assert_eq!(requests[1].model.as_deref(), Some("model-b"));
        assert_eq!(requests[2].model.as_deref(), Some("model-a"));
        assert_eq!(requests[3].model.as_deref(), Some("model-b"));
        assert_eq!(requests[4].model.as_deref(), Some("model-a"));
    }

    #[test]
    fn create_branch_requests_uses_default_model_when_none_provided() {
        let config = BranchConfig {
            parallel_count: 3,
            models: Vec::new(),
            auto_rank: false,
        };

        let requests = BranchManager::create_branch_requests(
            &config,
            "Solve this task",
            AgentRole::Implementation,
            "implement",
            sample_context(),
            "default-model",
        );

        assert_eq!(requests.len(), 3);
        assert!(requests
            .iter()
            .all(|request| request.model.as_deref() == Some("default-model")));
    }

    #[test]
    fn create_session_requires_at_least_two_branches() {
        let panic = std::panic::catch_unwind(|| {
            BranchManager::create_session(
                BranchConfig {
                    parallel_count: 1,
                    models: Vec::new(),
                    auto_rank: false,
                },
                "prompt".to_string(),
            )
        });

        assert!(panic.is_err());
    }

    #[test]
    fn create_session_starts_in_preparing_status() {
        let session = BranchManager::create_session(
            BranchConfig {
                parallel_count: 2,
                models: vec!["model-a".to_string(), "model-b".to_string()],
                auto_rank: false,
            },
            "Explore solution space".to_string(),
        );

        assert_eq!(session.status, BranchStatus::Preparing);
        assert_eq!(session.results.len(), 0);
        assert_eq!(session.branch_agent_ids.len(), 0);
    }

    #[test]
    fn record_spawn_moves_session_to_running() {
        let mut session = BranchManager::create_session(
            BranchConfig {
                parallel_count: 2,
                models: Vec::new(),
                auto_rank: false,
            },
            "prompt".to_string(),
        );

        BranchManager::record_spawn(&mut session, Uuid::new_v4());
        assert_eq!(session.status, BranchStatus::Running);
    }

    #[test]
    fn record_result_drives_completion_state() {
        let mut session = BranchManager::create_session(
            BranchConfig {
                parallel_count: 2,
                models: Vec::new(),
                auto_rank: false,
            },
            "prompt".to_string(),
        );

        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        BranchManager::record_spawn(&mut session, first);
        BranchManager::record_spawn(&mut session, second);

        BranchManager::record_result(
            &mut session,
            BranchResult {
                agent_id: first,
                model: "m1".to_string(),
                summary: "first".to_string(),
                token_usage: sample_tokens(10, 5, 0.01),
                success: true,
                rank: None,
            },
        );

        assert!(!BranchManager::is_complete(&session));
        assert_eq!(session.status, BranchStatus::Running);

        BranchManager::record_result(
            &mut session,
            BranchResult {
                agent_id: second,
                model: "m2".to_string(),
                summary: "second".to_string(),
                token_usage: sample_tokens(15, 8, 0.02),
                success: true,
                rank: None,
            },
        );

        assert!(BranchManager::is_complete(&session));
        assert_eq!(session.status, BranchStatus::Completed);
    }

    #[test]
    fn update_status_handles_partial_and_total_failure() {
        let mut partial = BranchManager::create_session(
            BranchConfig {
                parallel_count: 2,
                models: Vec::new(),
                auto_rank: false,
            },
            "prompt".to_string(),
        );
        BranchManager::record_result(
            &mut partial,
            BranchResult {
                agent_id: Uuid::new_v4(),
                model: "m1".to_string(),
                summary: "ok".to_string(),
                token_usage: sample_tokens(3, 2, 0.01),
                success: true,
                rank: None,
            },
        );
        BranchManager::record_result(
            &mut partial,
            BranchResult {
                agent_id: Uuid::new_v4(),
                model: "m2".to_string(),
                summary: "fail".to_string(),
                token_usage: sample_tokens(4, 3, 0.01),
                success: false,
                rank: None,
            },
        );
        assert_eq!(partial.status, BranchStatus::PartiallyCompleted);

        let mut failed = BranchManager::create_session(
            BranchConfig {
                parallel_count: 2,
                models: Vec::new(),
                auto_rank: false,
            },
            "prompt".to_string(),
        );
        BranchManager::record_result(
            &mut failed,
            BranchResult {
                agent_id: Uuid::new_v4(),
                model: "m1".to_string(),
                summary: "fail".to_string(),
                token_usage: sample_tokens(3, 2, 0.01),
                success: false,
                rank: None,
            },
        );
        BranchManager::record_result(
            &mut failed,
            BranchResult {
                agent_id: Uuid::new_v4(),
                model: "m2".to_string(),
                summary: "fail".to_string(),
                token_usage: sample_tokens(4, 3, 0.01),
                success: false,
                rank: None,
            },
        );
        assert_eq!(failed.status, BranchStatus::Failed);
    }

    #[test]
    fn ranked_results_use_rank_then_fallback_to_token_efficiency() {
        let mut ranked_session = BranchManager::create_session(
            BranchConfig {
                parallel_count: 3,
                models: Vec::new(),
                auto_rank: true,
            },
            "prompt".to_string(),
        );
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        BranchManager::record_result(
            &mut ranked_session,
            BranchResult {
                agent_id: a,
                model: "m1".to_string(),
                summary: "a".to_string(),
                token_usage: sample_tokens(10, 10, 0.10),
                success: true,
                rank: Some(2),
            },
        );
        BranchManager::record_result(
            &mut ranked_session,
            BranchResult {
                agent_id: b,
                model: "m2".to_string(),
                summary: "b".to_string(),
                token_usage: sample_tokens(10, 10, 0.08),
                success: true,
                rank: Some(1),
            },
        );
        BranchManager::record_result(
            &mut ranked_session,
            BranchResult {
                agent_id: c,
                model: "m3".to_string(),
                summary: "c".to_string(),
                token_usage: sample_tokens(10, 10, 0.05),
                success: true,
                rank: None,
            },
        );

        let ranked = BranchManager::ranked_results(&ranked_session);
        assert_eq!(ranked[0].agent_id, b);
        assert_eq!(ranked[1].agent_id, a);
        assert_eq!(ranked[2].agent_id, c);

        let mut fallback_session = BranchManager::create_session(
            BranchConfig {
                parallel_count: 3,
                models: Vec::new(),
                auto_rank: false,
            },
            "prompt".to_string(),
        );
        BranchManager::record_result(
            &mut fallback_session,
            BranchResult {
                agent_id: Uuid::new_v4(),
                model: "m1".to_string(),
                summary: "a".to_string(),
                token_usage: sample_tokens(10, 10, 0.20),
                success: true,
                rank: None,
            },
        );
        BranchManager::record_result(
            &mut fallback_session,
            BranchResult {
                agent_id: Uuid::new_v4(),
                model: "m2".to_string(),
                summary: "b".to_string(),
                token_usage: sample_tokens(10, 10, 0.05),
                success: true,
                rank: None,
            },
        );
        BranchManager::record_result(
            &mut fallback_session,
            BranchResult {
                agent_id: Uuid::new_v4(),
                model: "m3".to_string(),
                summary: "c".to_string(),
                token_usage: sample_tokens(10, 10, 0.10),
                success: true,
                rank: None,
            },
        );

        let fallback = BranchManager::ranked_results(&fallback_session);
        assert!(fallback[0].token_usage.cost_usd <= fallback[1].token_usage.cost_usd);
        assert!(fallback[1].token_usage.cost_usd <= fallback[2].token_usage.cost_usd);
    }

    #[test]
    fn total_tokens_sums_usage_across_branches() {
        let mut session = BranchManager::create_session(
            BranchConfig {
                parallel_count: 2,
                models: Vec::new(),
                auto_rank: false,
            },
            "prompt".to_string(),
        );
        BranchManager::record_result(
            &mut session,
            BranchResult {
                agent_id: Uuid::new_v4(),
                model: "m1".to_string(),
                summary: "a".to_string(),
                token_usage: sample_tokens(10, 20, 0.11),
                success: true,
                rank: None,
            },
        );
        BranchManager::record_result(
            &mut session,
            BranchResult {
                agent_id: Uuid::new_v4(),
                model: "m2".to_string(),
                summary: "b".to_string(),
                token_usage: sample_tokens(30, 40, 0.22),
                success: true,
                rank: None,
            },
        );

        let total = BranchManager::total_tokens(&session);
        assert_eq!(total.input, 40);
        assert_eq!(total.output, 60);
        assert!((total.cost_usd - 0.33).abs() < 1e-9);
    }
}
