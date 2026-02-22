use serde::{Deserialize, Serialize};
use std::ops::{Add, AddAssign};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Spawning,
    Running,
    WaitingOnUser,
    Reporting,
    Complete,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Orchestrator,
    Research,
    Implementation,
    Test,
    Review,
    Unstuck,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenMetrics {
    pub input: u64,
    pub output: u64,
    pub cost_usd: f64,
}

impl Add for TokenMetrics {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            input: self.input + rhs.input,
            output: self.output + rhs.output,
            cost_usd: self.cost_usd + rhs.cost_usd,
        }
    }
}

impl AddAssign for TokenMetrics {
    fn add_assign(&mut self, rhs: Self) {
        self.input += rhs.input;
        self.output += rhs.output;
        self.cost_usd += rhs.cost_usd;
    }
}

/// Placeholder for CompactedContext from the compaction sub-system (04).
/// Will be replaced with the real type when compaction is implemented.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactedContextRef {
    pub summary: String,
    pub token_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgent {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub role: AgentRole,
    pub model: String,
    pub mode: String,
    pub status: AgentStatus,
    pub context: Option<CompactedContextRef>,
    pub allowed_tools: Vec<String>,
    pub token_usage: TokenMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchConfig {
    pub parallel_count: usize,
    pub models: Vec<String>,
    pub auto_rank: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_metrics_add() {
        let a = TokenMetrics {
            input: 100,
            output: 50,
            cost_usd: 0.01,
        };
        let b = TokenMetrics {
            input: 200,
            output: 100,
            cost_usd: 0.02,
        };

        let c = a + b;
        assert_eq!(c.input, 300);
        assert_eq!(c.output, 150);
        assert!((c.cost_usd - 0.03).abs() < f64::EPSILON);
    }

    #[test]
    fn token_metrics_add_assign() {
        let mut a = TokenMetrics {
            input: 100,
            output: 50,
            cost_usd: 0.01,
        };

        a += TokenMetrics {
            input: 200,
            output: 100,
            cost_usd: 0.02,
        };

        assert_eq!(a.input, 300);
        assert_eq!(a.output, 150);
        assert!((a.cost_usd - 0.03).abs() < f64::EPSILON);
    }

    #[test]
    fn agent_role_serde_roundtrip() {
        let expectations = [
            (AgentRole::Orchestrator, "\"orchestrator\""),
            (AgentRole::Research, "\"research\""),
            (AgentRole::Implementation, "\"implementation\""),
            (AgentRole::Test, "\"test\""),
            (AgentRole::Review, "\"review\""),
            (AgentRole::Unstuck, "\"unstuck\""),
        ];

        for (role, expected_json) in expectations {
            let json = serde_json::to_string(&role).expect("role should serialize");
            assert_eq!(json, expected_json);
            let back: AgentRole = serde_json::from_str(&json).expect("role should deserialize");
            assert_eq!(role, back);
        }
    }

    #[test]
    fn agent_status_serde_roundtrip() {
        let expectations = [
            (AgentStatus::Spawning, "\"spawning\""),
            (AgentStatus::Running, "\"running\""),
            (AgentStatus::WaitingOnUser, "\"waiting_on_user\""),
            (AgentStatus::Reporting, "\"reporting\""),
            (AgentStatus::Complete, "\"complete\""),
            (AgentStatus::Failed, "\"failed\""),
        ];

        for (status, expected_json) in expectations {
            let json = serde_json::to_string(&status).expect("status should serialize");
            assert_eq!(json, expected_json);
            let back: AgentStatus = serde_json::from_str(&json).expect("status should deserialize");
            assert_eq!(status, back);
        }
    }

    #[test]
    fn sub_agent_serde_roundtrip() {
        let id = Uuid::parse_str("2f2b61d3-8ce2-4fda-b985-a16d4f976f75").unwrap();
        let parent_id = Uuid::parse_str("588f65bd-6b57-4f30-8bd5-0574f63b237f").unwrap();

        let original = SubAgent {
            id,
            parent_id: Some(parent_id),
            role: AgentRole::Implementation,
            model: "claude-sonnet-4-20250514".to_string(),
            mode: "implement".to_string(),
            status: AgentStatus::Running,
            context: Some(CompactedContextRef {
                summary: "Implement parser".to_string(),
                token_count: 128,
            }),
            allowed_tools: vec!["fs_read".to_string(), "fs_write".to_string()],
            token_usage: TokenMetrics {
                input: 120,
                output: 80,
                cost_usd: 0.012,
            },
        };

        let json = serde_json::to_string(&original).expect("sub-agent should serialize");
        let back: SubAgent = serde_json::from_str(&json).expect("sub-agent should deserialize");

        assert_eq!(back.id, original.id);
        assert_eq!(back.parent_id, original.parent_id);
        assert_eq!(back.role, original.role);
        assert_eq!(back.model, original.model);
        assert_eq!(back.mode, original.mode);
        assert_eq!(back.status, original.status);
        let back_context = back.context.expect("context should roundtrip");
        let original_context = original.context.expect("context should exist");
        assert_eq!(back_context.summary, original_context.summary);
        assert_eq!(back_context.token_count, original_context.token_count);
        assert_eq!(back.allowed_tools, original.allowed_tools);
        assert_eq!(back.token_usage.input, original.token_usage.input);
        assert_eq!(back.token_usage.output, original.token_usage.output);
        assert!((back.token_usage.cost_usd - original.token_usage.cost_usd).abs() < f64::EPSILON);
    }
}
