use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::types::{AgentRole, AgentStatus, SubAgent, TokenMetrics};

/// Events produced by lifecycle transitions.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    Spawned {
        agent_id: Uuid,
        role: AgentRole,
        model: String,
        parent_id: Option<Uuid>,
    },
    Completed {
        agent_id: Uuid,
        summary: String,
        token_usage: TokenMetrics,
    },
    Failed {
        agent_id: Uuid,
        error: String,
        token_usage: TokenMetrics,
    },
    Looped {
        agent_id: Uuid,
        repeated_tool: String,
        count: u32,
    },
}

#[derive(Debug, Clone)]
pub enum TransitionContext {
    None,
    Completed { summary: String },
    Failed { error: String },
    Looped { repeated_tool: String, count: u32 },
}

/// Validate that a status transition is legal.
pub fn validate_transition(from: &AgentStatus, to: &AgentStatus) -> Result<(), String> {
    debug!(?from, ?to, "validating transition");
    match (from, to) {
        (AgentStatus::Spawning, AgentStatus::Running)
        | (AgentStatus::Running, AgentStatus::Reporting)
        | (AgentStatus::Running, AgentStatus::Failed)
        | (AgentStatus::Running, AgentStatus::WaitingOnUser)
        | (AgentStatus::WaitingOnUser, AgentStatus::Running)
        | (AgentStatus::Reporting, AgentStatus::Complete)
        | (AgentStatus::Reporting, AgentStatus::Failed) => Ok(()),
        _ => {
            warn!(?from, ?to, "invalid agent status transition");
            Err(format!(
                "invalid agent status transition: {:?} -> {:?}",
                from, to
            ))
        }
    }
}

pub struct AgentLifecycleManager;

impl AgentLifecycleManager {
    /// Transition an agent to a new status, returning the event to emit.
    /// Mutates the agent's status field in place.
    /// Returns Err if the transition is invalid.
    pub fn transition(
        agent: &mut SubAgent,
        to: AgentStatus,
        event_context: TransitionContext,
    ) -> Result<Option<AgentEvent>, String> {
        let from = agent.status.clone();
        info!(agent_id = %agent.id, ?from, ?to, "agent transition");

        match event_context {
            TransitionContext::Looped {
                repeated_tool,
                count,
            } => {
                if !matches!(from, AgentStatus::Running) {
                    error!(agent_id = %agent.id, ?from, "looped event requires agent to be Running");
                    return Err(format!(
                        "looped event requires agent to be Running, current status: {:?}",
                        from
                    ));
                }

                if !matches!(to, AgentStatus::Running) {
                    error!(agent_id = %agent.id, ?to, "looped event requires target status Running");
                    return Err(format!(
                        "looped event requires target status Running, got: {:?}",
                        to
                    ));
                }

                return Ok(Some(AgentEvent::Looped {
                    agent_id: agent.id,
                    repeated_tool,
                    count,
                }));
            }
            TransitionContext::Failed { error } => {
                if !matches!(to, AgentStatus::Failed) {
                    error!(agent_id = %agent.id, ?to, "failed context requires target status Failed");
                    return Err(format!(
                        "failed context requires target status Failed, got: {:?}",
                        to
                    ));
                }

                agent.status = AgentStatus::Failed;
                return Ok(Some(AgentEvent::Failed {
                    agent_id: agent.id,
                    error,
                    token_usage: agent.token_usage.clone(),
                }));
            }
            TransitionContext::Completed { summary } => {
                if !matches!(to, AgentStatus::Complete) {
                    error!(agent_id = %agent.id, ?to, "completed context requires target status Complete");
                    return Err(format!(
                        "completed context requires target status Complete, got: {:?}",
                        to
                    ));
                }

                validate_transition(&from, &to)?;
                agent.status = to;
                return Ok(Some(AgentEvent::Completed {
                    agent_id: agent.id,
                    summary,
                    token_usage: agent.token_usage.clone(),
                }));
            }
            TransitionContext::None => {}
        }

        validate_transition(&from, &to)?;
        let emit_event = matches!((&from, &to), (AgentStatus::Spawning, AgentStatus::Running));
        agent.status = to;

        if emit_event {
            Ok(Some(AgentEvent::Spawned {
                agent_id: agent.id,
                role: agent.role.clone(),
                model: agent.model.clone(),
                parent_id: agent.parent_id,
            }))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_agent(status: AgentStatus) -> SubAgent {
        SubAgent {
            id: Uuid::new_v4(),
            parent_id: Some(Uuid::new_v4()),
            role: AgentRole::Implementation,
            model: "test-model".to_string(),
            mode: "code".to_string(),
            status,
            context: None,
            allowed_tools: vec!["read_file".to_string()],
            token_usage: TokenMetrics {
                input: 10,
                output: 20,
                cost_usd: 0.1,
            },
        }
    }

    #[test]
    fn validate_transition_accepts_legal_transitions() {
        let legal = [
            (AgentStatus::Spawning, AgentStatus::Running),
            (AgentStatus::Running, AgentStatus::Reporting),
            (AgentStatus::Running, AgentStatus::Failed),
            (AgentStatus::Running, AgentStatus::WaitingOnUser),
            (AgentStatus::WaitingOnUser, AgentStatus::Running),
            (AgentStatus::Reporting, AgentStatus::Complete),
            (AgentStatus::Reporting, AgentStatus::Failed),
        ];

        for (from, to) in legal {
            assert!(validate_transition(&from, &to).is_ok());
        }
    }

    #[test]
    fn validate_transition_rejects_illegal_transitions() {
        let illegal = [
            (AgentStatus::Spawning, AgentStatus::Complete),
            (AgentStatus::WaitingOnUser, AgentStatus::Reporting),
            (AgentStatus::Complete, AgentStatus::Running),
            (AgentStatus::Failed, AgentStatus::Running),
            (AgentStatus::Running, AgentStatus::Complete),
        ];

        for (from, to) in illegal {
            assert!(validate_transition(&from, &to).is_err());
        }
    }

    #[test]
    fn transition_spawn_to_running_emits_spawned_event() {
        let mut agent = sample_agent(AgentStatus::Spawning);
        let role = agent.role.clone();
        let model = agent.model.clone();
        let parent_id = agent.parent_id;
        let agent_id = agent.id;

        let event = AgentLifecycleManager::transition(
            &mut agent,
            AgentStatus::Running,
            TransitionContext::None,
        )
        .unwrap()
        .unwrap();

        assert!(matches!(agent.status, AgentStatus::Running));
        match event {
            AgentEvent::Spawned {
                agent_id: spawned_id,
                role: spawned_role,
                model: spawned_model,
                parent_id: spawned_parent,
            } => {
                assert_eq!(spawned_id, agent_id);
                assert_eq!(spawned_role, role);
                assert_eq!(spawned_model, model);
                assert_eq!(spawned_parent, parent_id);
            }
            _ => panic!("expected spawned event"),
        }
    }

    #[test]
    fn transition_running_to_reporting_emits_no_event() {
        let mut agent = sample_agent(AgentStatus::Running);
        let event = AgentLifecycleManager::transition(
            &mut agent,
            AgentStatus::Reporting,
            TransitionContext::None,
        )
        .unwrap();

        assert!(event.is_none());
        assert!(matches!(agent.status, AgentStatus::Reporting));
    }

    #[test]
    fn transition_reporting_to_complete_emits_completed_event() {
        let mut agent = sample_agent(AgentStatus::Reporting);
        let event = AgentLifecycleManager::transition(
            &mut agent,
            AgentStatus::Complete,
            TransitionContext::Completed {
                summary: "done".to_string(),
            },
        )
        .unwrap()
        .unwrap();

        assert!(matches!(agent.status, AgentStatus::Complete));
        match event {
            AgentEvent::Completed {
                summary,
                token_usage,
                ..
            } => {
                assert_eq!(summary, "done");
                assert_eq!(token_usage.input, 10);
                assert_eq!(token_usage.output, 20);
                assert!((token_usage.cost_usd - 0.1).abs() < f64::EPSILON);
            }
            _ => panic!("expected completed event"),
        }
    }

    #[test]
    fn transition_running_to_failed_emits_failed_event() {
        let mut agent = sample_agent(AgentStatus::Running);
        let event = AgentLifecycleManager::transition(
            &mut agent,
            AgentStatus::Failed,
            TransitionContext::Failed {
                error: "tool timeout".to_string(),
            },
        )
        .unwrap()
        .unwrap();

        assert!(matches!(agent.status, AgentStatus::Failed));
        match event {
            AgentEvent::Failed {
                error, token_usage, ..
            } => {
                assert_eq!(error, "tool timeout");
                assert_eq!(token_usage.input, 10);
                assert_eq!(token_usage.output, 20);
                assert!((token_usage.cost_usd - 0.1).abs() < f64::EPSILON);
            }
            _ => panic!("expected failed event"),
        }
    }

    #[test]
    fn transition_any_to_failed_emits_failed_event() {
        let mut agent = sample_agent(AgentStatus::Spawning);
        let event = AgentLifecycleManager::transition(
            &mut agent,
            AgentStatus::Failed,
            TransitionContext::Failed {
                error: "boom".to_string(),
            },
        )
        .unwrap()
        .unwrap();

        assert!(matches!(agent.status, AgentStatus::Failed));
        match event {
            AgentEvent::Failed {
                error, token_usage, ..
            } => {
                assert_eq!(error, "boom");
                assert_eq!(token_usage.input, 10);
                assert_eq!(token_usage.output, 20);
                assert!((token_usage.cost_usd - 0.1).abs() < f64::EPSILON);
            }
            _ => panic!("expected failed event"),
        }
    }

    #[test]
    fn transition_invalid_does_not_mutate_status() {
        let mut agent = sample_agent(AgentStatus::Spawning);
        let result = AgentLifecycleManager::transition(
            &mut agent,
            AgentStatus::Complete,
            TransitionContext::None,
        );

        assert!(result.is_err());
        assert!(matches!(agent.status, AgentStatus::Spawning));
    }

    #[test]
    fn transition_looped_emits_event_without_status_change() {
        let mut agent = sample_agent(AgentStatus::Running);
        let event = AgentLifecycleManager::transition(
            &mut agent,
            AgentStatus::Running,
            TransitionContext::Looped {
                repeated_tool: "read_file".to_string(),
                count: 3,
            },
        )
        .unwrap()
        .unwrap();

        assert!(matches!(agent.status, AgentStatus::Running));
        match event {
            AgentEvent::Looped {
                repeated_tool,
                count,
                ..
            } => {
                assert_eq!(repeated_tool, "read_file");
                assert_eq!(count, 3);
            }
            _ => panic!("expected looped event"),
        }
    }
}
