use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::tree::AgentTree;
use super::types::{AgentRole, AgentStatus, CompactedContextRef, SubAgent, TokenMetrics};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnRequest {
    pub role: AgentRole,
    pub model: Option<String>,
    pub mode: String,
    pub context: CompactedContextRef,
    pub allowed_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentReport {
    pub agent_id: Uuid,
    pub summary: String,
    pub token_usage: TokenMetrics,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubTaskDescription {
    pub title: String,
    pub description: String,
    pub suggested_role: AgentRole,
    pub suggested_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleModelDefaults {
    pub orchestrator: String,
    pub research: String,
    pub implementation: String,
    pub review: String,
    pub test: String,
    pub unstuck: String,
}

impl RoleModelDefaults {
    pub fn model_for_role(&self, role: &AgentRole) -> &str {
        match role {
            AgentRole::Orchestrator => &self.orchestrator,
            AgentRole::Research => &self.research,
            AgentRole::Implementation => &self.implementation,
            AgentRole::Review => &self.review,
            AgentRole::Test => &self.test,
            AgentRole::Unstuck => &self.unstuck,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Orchestrator {
    tree: AgentTree,
    session_id: Uuid,
}

impl Orchestrator {
    pub fn new(session_id: Uuid) -> Self {
        info!(%session_id, "creating orchestrator");
        let mut tree = AgentTree::new();
        tree.set_root(SubAgent {
            id: session_id,
            parent_id: None,
            role: AgentRole::Orchestrator,
            model: "orchestrator".to_string(),
            mode: "orchestrator".to_string(),
            status: AgentStatus::Running,
            context: None,
            allowed_tools: Vec::new(),
            token_usage: TokenMetrics::default(),
        });

        Self { tree, session_id }
    }

    pub fn plan_spawn_requests(
        &self,
        subtasks: Vec<SubTaskDescription>,
        role_defaults: &RoleModelDefaults,
    ) -> Vec<SpawnRequest> {
        debug!(count = subtasks.len(), "planning spawn requests");
        subtasks
            .into_iter()
            .map(|task| SpawnRequest {
                model: Some(
                    role_defaults
                        .model_for_role(&task.suggested_role)
                        .to_string(),
                ),
                role: task.suggested_role,
                mode: task.suggested_mode,
                context: CompactedContextRef {
                    summary: format!("{}: {}", task.title, task.description),
                    token_count: 0,
                },
                allowed_tools: Vec::new(),
            })
            .collect()
    }

    pub fn spawn(&mut self, parent_id: Uuid, request: SpawnRequest) -> Uuid {
        let agent_id = Uuid::new_v4();
        info!(%parent_id, role = ?request.role, %agent_id, "spawning agent");
        let default_model = default_model_for_role(&request.role);

        let agent = SubAgent {
            id: agent_id,
            parent_id: Some(parent_id),
            role: request.role,
            model: request.model.unwrap_or_else(|| default_model.to_string()),
            mode: request.mode,
            status: AgentStatus::Spawning,
            context: Some(request.context),
            allowed_tools: request.allowed_tools,
            token_usage: TokenMetrics::default(),
        };

        self.tree.add_child(parent_id, agent);
        agent_id
    }

    pub fn collect_report(&mut self, report: AgentReport) {
        info!(agent_id = %report.agent_id, success = report.success, "collecting agent report");
        if let Some(agent) = self.tree.get_mut(&report.agent_id) {
            agent.token_usage = report.token_usage;
            agent.status = if report.success {
                AgentStatus::Complete
            } else {
                AgentStatus::Failed
            };
        } else {
            warn!(agent_id = %report.agent_id, "agent not found when collecting report");
        }
    }

    pub fn completed_reports(&self, parent_id: &Uuid) -> Vec<&SubAgent> {
        let reports: Vec<&SubAgent> = self
            .tree
            .children_of(parent_id)
            .into_iter()
            .filter(|agent| matches!(agent.status, AgentStatus::Complete | AgentStatus::Failed))
            .collect();
        debug!(%parent_id, count = reports.len(), "completed reports");
        reports
    }

    pub fn all_children_done(&self, parent_id: &Uuid) -> bool {
        let done = self
            .tree
            .children_of(parent_id)
            .into_iter()
            .all(|child| matches!(child.status, AgentStatus::Complete | AgentStatus::Failed));
        debug!(%parent_id, all_done = done, "all children done check");
        done
    }

    pub fn tree(&self) -> &AgentTree {
        &self.tree
    }

    pub fn total_tokens(&self) -> TokenMetrics {
        let totals = self
            .tree
            .all_agents()
            .into_iter()
            .fold(TokenMetrics::default(), |acc, agent| {
                acc + agent.token_usage.clone()
            });
        debug!(
            input = totals.input,
            output = totals.output,
            cost_usd = totals.cost_usd,
            "total tokens"
        );
        totals
    }

    pub fn session_id(&self) -> Uuid {
        self.session_id
    }
}

fn default_model_for_role(role: &AgentRole) -> &'static str {
    match role {
        AgentRole::Orchestrator => "default-orchestrator-model",
        AgentRole::Research => "default-research-model",
        AgentRole::Implementation => "default-implementation-model",
        AgentRole::Review => "default-review-model",
        AgentRole::Test => "default-test-model",
        AgentRole::Unstuck => "default-unstuck-model",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role_defaults() -> RoleModelDefaults {
        RoleModelDefaults {
            orchestrator: "claude-sonnet-4-20250514".to_string(),
            research: "claude-haiku-4-20250514".to_string(),
            implementation: "claude-sonnet-4-20250514".to_string(),
            review: "claude-sonnet-4-20250514".to_string(),
            test: "claude-haiku-4-20250514".to_string(),
            unstuck: "claude-opus-4-20250514".to_string(),
        }
    }

    #[test]
    fn plan_spawn_requests_maps_role_defaults() {
        let orchestrator = Orchestrator::new(Uuid::new_v4());
        let requests = orchestrator.plan_spawn_requests(
            vec![
                SubTaskDescription {
                    title: "Research API".to_string(),
                    description: "Find endpoint details".to_string(),
                    suggested_role: AgentRole::Research,
                    suggested_mode: "research".to_string(),
                },
                SubTaskDescription {
                    title: "Implement parser".to_string(),
                    description: "Build JSON parser".to_string(),
                    suggested_role: AgentRole::Implementation,
                    suggested_mode: "implementation".to_string(),
                },
            ],
            &role_defaults(),
        );

        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].model.as_deref(),
            Some("claude-haiku-4-20250514")
        );
        assert_eq!(
            requests[1].model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        assert_eq!(requests[0].role, AgentRole::Research);
        assert_eq!(requests[1].role, AgentRole::Implementation);
    }

    #[test]
    fn spawn_adds_agent_under_parent() {
        let session_id = Uuid::new_v4();
        let mut orchestrator = Orchestrator::new(session_id);
        let child_id = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Implementation,
                model: None,
                mode: "implementation".to_string(),
                context: CompactedContextRef {
                    summary: "Implement a feature".to_string(),
                    token_count: 100,
                },
                allowed_tools: vec!["read".to_string(), "write".to_string()],
            },
        );

        let child = orchestrator
            .tree()
            .get(&child_id)
            .expect("spawned child should be in tree");
        assert_eq!(child.parent_id, Some(session_id));
        assert_eq!(child.role, AgentRole::Implementation);
        assert_eq!(child.status, AgentStatus::Spawning);
    }

    #[test]
    fn spawn_sets_correct_fields() {
        let session_id = Uuid::new_v4();
        let mut orchestrator = Orchestrator::new(session_id);
        let child_id = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Test,
                model: None,
                mode: "implement".to_string(),
                context: CompactedContextRef {
                    summary: "Write regression tests".to_string(),
                    token_count: 34,
                },
                allowed_tools: vec!["fs_read".to_string(), "fs_write".to_string()],
            },
        );

        let child = orchestrator
            .tree()
            .get(&child_id)
            .expect("spawned child should be present");
        assert_eq!(child.parent_id, Some(session_id));
        assert_eq!(child.role, AgentRole::Test);
        assert_eq!(child.model, "default-test-model");
        assert_eq!(child.mode, "implement");
        assert_eq!(child.allowed_tools, vec!["fs_read", "fs_write"]);
        let context = child.context.as_ref().expect("context should be set");
        assert_eq!(context.summary, "Write regression tests");
        assert_eq!(context.token_count, 34);
    }

    #[test]
    fn collect_report_updates_status_and_tokens() {
        let session_id = Uuid::new_v4();
        let mut orchestrator = Orchestrator::new(session_id);
        let child_id = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Review,
                model: Some("claude-sonnet-4-20250514".to_string()),
                mode: "review".to_string(),
                context: CompactedContextRef {
                    summary: "Review patch".to_string(),
                    token_count: 45,
                },
                allowed_tools: vec!["read".to_string()],
            },
        );

        orchestrator.collect_report(AgentReport {
            agent_id: child_id,
            summary: "Looks good".to_string(),
            token_usage: TokenMetrics {
                input: 120,
                output: 80,
                cost_usd: 0.012,
            },
            success: true,
            error: None,
        });

        let child = orchestrator
            .tree()
            .get(&child_id)
            .expect("child should exist");
        assert_eq!(child.status, AgentStatus::Complete);
        assert_eq!(child.token_usage.input, 120);
        assert_eq!(child.token_usage.output, 80);
        assert!((child.token_usage.cost_usd - 0.012).abs() < f64::EPSILON);
    }

    #[test]
    fn collect_report_failure_marks_agent_failed() {
        let session_id = Uuid::new_v4();
        let mut orchestrator = Orchestrator::new(session_id);
        let child_id = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Research,
                model: Some("claude-haiku-4-20250514".to_string()),
                mode: "research".to_string(),
                context: CompactedContextRef {
                    summary: "Investigate issue".to_string(),
                    token_count: 20,
                },
                allowed_tools: vec!["web_search".to_string()],
            },
        );

        orchestrator.collect_report(AgentReport {
            agent_id: child_id,
            summary: "Could not resolve".to_string(),
            token_usage: TokenMetrics {
                input: 40,
                output: 12,
                cost_usd: 0.003,
            },
            success: false,
            error: Some("rate limited".to_string()),
        });

        let child = orchestrator
            .tree()
            .get(&child_id)
            .expect("child should exist");
        assert_eq!(child.status, AgentStatus::Failed);
        assert_eq!(child.token_usage.input, 40);
        assert_eq!(child.token_usage.output, 12);
        assert!((child.token_usage.cost_usd - 0.003).abs() < f64::EPSILON);
    }

    #[test]
    fn all_children_done_is_true_only_when_all_terminal() {
        let session_id = Uuid::new_v4();
        let mut orchestrator = Orchestrator::new(session_id);
        let first = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Research,
                model: Some("claude-haiku-4-20250514".to_string()),
                mode: "research".to_string(),
                context: CompactedContextRef {
                    summary: "Gather context".to_string(),
                    token_count: 22,
                },
                allowed_tools: Vec::new(),
            },
        );
        let second = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Test,
                model: Some("claude-haiku-4-20250514".to_string()),
                mode: "test".to_string(),
                context: CompactedContextRef {
                    summary: "Write tests".to_string(),
                    token_count: 10,
                },
                allowed_tools: Vec::new(),
            },
        );

        assert!(!orchestrator.all_children_done(&session_id));

        orchestrator.collect_report(AgentReport {
            agent_id: first,
            summary: "done".to_string(),
            token_usage: TokenMetrics::default(),
            success: true,
            error: None,
        });
        assert!(!orchestrator.all_children_done(&session_id));

        orchestrator.collect_report(AgentReport {
            agent_id: second,
            summary: "failed".to_string(),
            token_usage: TokenMetrics::default(),
            success: false,
            error: Some("network".to_string()),
        });
        assert!(orchestrator.all_children_done(&session_id));
    }

    #[test]
    fn all_children_done_all_complete_returns_true() {
        let session_id = Uuid::new_v4();
        let mut orchestrator = Orchestrator::new(session_id);
        let first = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Research,
                model: Some("m".to_string()),
                mode: "research".to_string(),
                context: CompactedContextRef {
                    summary: "a".to_string(),
                    token_count: 1,
                },
                allowed_tools: Vec::new(),
            },
        );
        let second = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Implementation,
                model: Some("m".to_string()),
                mode: "implement".to_string(),
                context: CompactedContextRef {
                    summary: "b".to_string(),
                    token_count: 1,
                },
                allowed_tools: Vec::new(),
            },
        );
        let third = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Test,
                model: Some("m".to_string()),
                mode: "implement".to_string(),
                context: CompactedContextRef {
                    summary: "c".to_string(),
                    token_count: 1,
                },
                allowed_tools: Vec::new(),
            },
        );

        for agent_id in [first, second, third] {
            orchestrator.collect_report(AgentReport {
                agent_id,
                summary: "ok".to_string(),
                token_usage: TokenMetrics::default(),
                success: true,
                error: None,
            });
        }

        assert!(orchestrator.all_children_done(&session_id));
    }

    #[test]
    fn all_children_done_some_running_returns_false() {
        let session_id = Uuid::new_v4();
        let mut orchestrator = Orchestrator::new(session_id);
        let completed = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Research,
                model: Some("m".to_string()),
                mode: "research".to_string(),
                context: CompactedContextRef {
                    summary: "a".to_string(),
                    token_count: 1,
                },
                allowed_tools: Vec::new(),
            },
        );
        let _running = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Implementation,
                model: Some("m".to_string()),
                mode: "implement".to_string(),
                context: CompactedContextRef {
                    summary: "b".to_string(),
                    token_count: 1,
                },
                allowed_tools: Vec::new(),
            },
        );

        orchestrator.collect_report(AgentReport {
            agent_id: completed,
            summary: "ok".to_string(),
            token_usage: TokenMetrics::default(),
            success: true,
            error: None,
        });

        assert!(!orchestrator.all_children_done(&session_id));
    }

    #[test]
    fn all_children_done_complete_and_failed_returns_true() {
        let session_id = Uuid::new_v4();
        let mut orchestrator = Orchestrator::new(session_id);
        let completed = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Research,
                model: Some("m".to_string()),
                mode: "research".to_string(),
                context: CompactedContextRef {
                    summary: "a".to_string(),
                    token_count: 1,
                },
                allowed_tools: Vec::new(),
            },
        );
        let failed = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Implementation,
                model: Some("m".to_string()),
                mode: "implement".to_string(),
                context: CompactedContextRef {
                    summary: "b".to_string(),
                    token_count: 1,
                },
                allowed_tools: Vec::new(),
            },
        );

        orchestrator.collect_report(AgentReport {
            agent_id: completed,
            summary: "ok".to_string(),
            token_usage: TokenMetrics::default(),
            success: true,
            error: None,
        });
        orchestrator.collect_report(AgentReport {
            agent_id: failed,
            summary: "err".to_string(),
            token_usage: TokenMetrics::default(),
            success: false,
            error: Some("failure".to_string()),
        });

        assert!(orchestrator.all_children_done(&session_id));
    }

    #[test]
    fn completed_reports_only_returns_terminal_children() {
        let session_id = Uuid::new_v4();
        let mut orchestrator = Orchestrator::new(session_id);
        let done_agent = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Research,
                model: Some("m".to_string()),
                mode: "research".to_string(),
                context: CompactedContextRef {
                    summary: "done".to_string(),
                    token_count: 1,
                },
                allowed_tools: Vec::new(),
            },
        );
        let running_agent = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Implementation,
                model: Some("m".to_string()),
                mode: "implementation".to_string(),
                context: CompactedContextRef {
                    summary: "running".to_string(),
                    token_count: 1,
                },
                allowed_tools: Vec::new(),
            },
        );

        orchestrator.collect_report(AgentReport {
            agent_id: done_agent,
            summary: "ok".to_string(),
            token_usage: TokenMetrics::default(),
            success: true,
            error: None,
        });

        let reports = orchestrator.completed_reports(&session_id);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].id, done_agent);
        assert!(orchestrator.tree().get(&running_agent).is_some());
    }

    #[test]
    fn total_tokens_sums_all_agents() {
        let session_id = Uuid::new_v4();
        let mut orchestrator = Orchestrator::new(session_id);
        let child_id = orchestrator.spawn(
            session_id,
            SpawnRequest {
                role: AgentRole::Implementation,
                model: Some("m".to_string()),
                mode: "implementation".to_string(),
                context: CompactedContextRef {
                    summary: "work".to_string(),
                    token_count: 5,
                },
                allowed_tools: Vec::new(),
            },
        );

        orchestrator.collect_report(AgentReport {
            agent_id: child_id,
            summary: "complete".to_string(),
            token_usage: TokenMetrics {
                input: 200,
                output: 150,
                cost_usd: 0.05,
            },
            success: true,
            error: None,
        });

        let total = orchestrator.total_tokens();
        assert_eq!(total.input, 200);
        assert_eq!(total.output, 150);
        assert!((total.cost_usd - 0.05).abs() < f64::EPSILON);
    }
}
