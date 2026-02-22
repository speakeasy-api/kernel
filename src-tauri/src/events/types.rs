use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// -- Supporting Types --

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Orchestrator,
    Implementation,
    Review,
    Planning,
    Research,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenMetrics {
    pub input: u64,
    pub output: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffStat {
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
}

// -- Event Core --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMetadata {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub session_id: Uuid,
    pub agent_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub metadata: EventMetadata,
    #[serde(flatten)]
    pub data: EventData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum EventData {
    // Prompt
    PromptSubmitted {
        prompt: String,
    },
    PromptClassified {
        mode: String,
        model: String,
        confidence: f32,
    },
    ModeOverridden {
        from_mode: String,
        to_mode: String,
    },

    // Agent
    AgentSpawned {
        agent_id: Uuid,
        role: AgentRole,
        model: String,
        parent_id: Option<Uuid>,
    },
    AgentCompleted {
        agent_id: Uuid,
        summary: String,
        token_usage: TokenMetrics,
    },
    AgentFailed {
        agent_id: Uuid,
        error: String,
        token_usage: TokenMetrics,
    },
    AgentLooped {
        agent_id: Uuid,
        repeated_tool: String,
        count: u32,
    },
    AgentSteered {
        agent_id: Uuid,
        instruction: String,
    },

    // Task
    TaskCreated {
        task_id: Uuid,
        title: String,
        parent_task: Option<Uuid>,
    },
    TaskStarted {
        task_id: Uuid,
        agent_id: Uuid,
    },
    TaskCompleted {
        task_id: Uuid,
        summary: String,
        diff_stat: DiffStat,
    },
    TaskBlocked {
        task_id: Uuid,
        reason: String,
        blocked_by: Vec<Uuid>,
    },
    TaskFailed {
        task_id: Uuid,
        reason: String,
    },

    // Review
    PlanAccepted {
        task_id: Uuid,
    },
    PlanRejected {
        task_id: Uuid,
        feedback: String,
    },
    DiffAccepted {
        task_id: Uuid,
        branch: String,
    },
    DiffRejected {
        task_id: Uuid,
        branch: String,
        feedback: String,
    },
    HunkRejected {
        task_id: Uuid,
        file: String,
        hunk_index: u32,
        reason: String,
    },

    // Tool
    ToolCalled {
        agent_id: Uuid,
        tool: String,
        args_summary: String,
    },
    ToolSucceeded {
        agent_id: Uuid,
        tool: String,
        duration_ms: u64,
    },
    ToolFailed {
        agent_id: Uuid,
        tool: String,
        error: String,
    },
    ToolRetried {
        agent_id: Uuid,
        tool: String,
        attempt: u32,
    },

    // Model
    ModelSelected {
        agent_id: Uuid,
        model: String,
        reason: String,
    },
    ModelSwitchedMidRun {
        agent_id: Uuid,
        from_model: String,
        to_model: String,
        reason: String,
    },
    TokensUsed {
        agent_id: Uuid,
        model: String,
        input: u64,
        output: u64,
    },
    CostIncurred {
        agent_id: Uuid,
        model: String,
        cost_usd: f64,
    },

    // Compaction
    ContextCompacted {
        agent_id: Uuid,
        before_tokens: usize,
        after_tokens: usize,
        regime: String,
    },
    LearningExtracted {
        agent_id: Uuid,
        learning: String,
    },
    FactsPreserved {
        agent_id: Uuid,
        facts: Vec<String>,
    },

    // UX
    ModeCreated {
        mode_name: String,
        trigger_pattern: String,
    },
    RecommendationSurfaced {
        recommendation_id: u64,
        summary: String,
    },
    RecommendationApplied {
        recommendation_id: u64,
    },
    RecommendationDismissed {
        recommendation_id: u64,
    },
    WarningShown {
        message: String,
        severity: String,
    },
}

impl EventData {
    pub fn kind(&self) -> &str {
        match self {
            Self::PromptSubmitted { .. } => "PromptSubmitted",
            Self::PromptClassified { .. } => "PromptClassified",
            Self::ModeOverridden { .. } => "ModeOverridden",
            Self::AgentSpawned { .. } => "AgentSpawned",
            Self::AgentCompleted { .. } => "AgentCompleted",
            Self::AgentFailed { .. } => "AgentFailed",
            Self::AgentLooped { .. } => "AgentLooped",
            Self::AgentSteered { .. } => "AgentSteered",
            Self::TaskCreated { .. } => "TaskCreated",
            Self::TaskStarted { .. } => "TaskStarted",
            Self::TaskCompleted { .. } => "TaskCompleted",
            Self::TaskBlocked { .. } => "TaskBlocked",
            Self::TaskFailed { .. } => "TaskFailed",
            Self::PlanAccepted { .. } => "PlanAccepted",
            Self::PlanRejected { .. } => "PlanRejected",
            Self::DiffAccepted { .. } => "DiffAccepted",
            Self::DiffRejected { .. } => "DiffRejected",
            Self::HunkRejected { .. } => "HunkRejected",
            Self::ToolCalled { .. } => "ToolCalled",
            Self::ToolSucceeded { .. } => "ToolSucceeded",
            Self::ToolFailed { .. } => "ToolFailed",
            Self::ToolRetried { .. } => "ToolRetried",
            Self::ModelSelected { .. } => "ModelSelected",
            Self::ModelSwitchedMidRun { .. } => "ModelSwitchedMidRun",
            Self::TokensUsed { .. } => "TokensUsed",
            Self::CostIncurred { .. } => "CostIncurred",
            Self::ContextCompacted { .. } => "ContextCompacted",
            Self::LearningExtracted { .. } => "LearningExtracted",
            Self::FactsPreserved { .. } => "FactsPreserved",
            Self::ModeCreated { .. } => "ModeCreated",
            Self::RecommendationSurfaced { .. } => "RecommendationSurfaced",
            Self::RecommendationApplied { .. } => "RecommendationApplied",
            Self::RecommendationDismissed { .. } => "RecommendationDismissed",
            Self::WarningShown { .. } => "WarningShown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_data_kind_returns_variant_name() {
        let data = EventData::PromptSubmitted {
            prompt: "hello".into(),
        };
        assert_eq!(data.kind(), "PromptSubmitted");

        let data = EventData::CostIncurred {
            agent_id: Uuid::new_v4(),
            model: "claude-sonnet-4-20250514".into(),
            cost_usd: 0.05,
        };
        assert_eq!(data.kind(), "CostIncurred");
    }

    #[test]
    fn event_data_serde_tag_content() {
        let data = EventData::PromptSubmitted {
            prompt: "hello".into(),
        };
        let json = serde_json::to_value(&data).unwrap();
        assert_eq!(json["kind"], "PromptSubmitted");
        assert_eq!(json["data"]["prompt"], "hello");
    }

    #[test]
    fn event_data_serde_roundtrip() {
        let id = Uuid::new_v4();
        let data = EventData::AgentSpawned {
            agent_id: id,
            role: AgentRole::Implementation,
            model: "claude-sonnet-4-20250514".into(),
            parent_id: None,
        };
        let json = serde_json::to_string(&data).unwrap();
        let parsed: EventData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.kind(), "AgentSpawned");
    }

    #[test]
    fn event_data_kind_matches_serde_tag() {
        let variants: Vec<EventData> = vec![
            EventData::PromptSubmitted { prompt: "x".into() },
            EventData::TaskFailed {
                task_id: Uuid::new_v4(),
                reason: "err".into(),
            },
            EventData::WarningShown {
                message: "w".into(),
                severity: "low".into(),
            },
        ];
        for variant in &variants {
            let json = serde_json::to_value(variant).unwrap();
            let serde_kind = json["kind"].as_str().unwrap();
            assert_eq!(variant.kind(), serde_kind);
        }
    }

    #[test]
    fn event_full_serde_roundtrip() {
        let event = Event {
            metadata: EventMetadata {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                session_id: Uuid::new_v4(),
                agent_id: Some(Uuid::new_v4()),
            },
            data: EventData::ToolSucceeded {
                agent_id: Uuid::new_v4(),
                tool: "read_file".into(),
                duration_ms: 42,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.metadata.id, event.metadata.id);
        assert_eq!(parsed.data.kind(), "ToolSucceeded");
    }

    #[test]
    fn token_metrics_serde() {
        let m = TokenMetrics {
            input: 100,
            output: 50,
        };
        let json = serde_json::to_string(&m).unwrap();
        let parsed: TokenMetrics = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, m);
    }

    #[test]
    fn diff_stat_serde() {
        let d = DiffStat {
            files_changed: 3,
            insertions: 40,
            deletions: 10,
        };
        let json = serde_json::to_string(&d).unwrap();
        let parsed: DiffStat = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, d);
    }

    #[test]
    fn agent_role_serde_snake_case() {
        let role = AgentRole::Implementation;
        let json = serde_json::to_value(&role).unwrap();
        assert_eq!(json, "implementation");
    }
}
