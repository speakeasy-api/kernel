use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

// -- Enums --

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Blocked,
    Review,
    Done,
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::InProgress => write!(f, "in_progress"),
            Self::Blocked => write!(f, "blocked"),
            Self::Review => write!(f, "review"),
            Self::Done => write!(f, "done"),
        }
    }
}

impl FromStr for TaskStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "in_progress" => Ok(Self::InProgress),
            "blocked" => Ok(Self::Blocked),
            "review" => Ok(Self::Review),
            "done" => Ok(Self::Done),
            _ => Err(format!("unknown task status: {s}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    Medium,
    High,
    Critical,
}

impl Priority {
    pub fn as_i32(&self) -> i32 {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
            Self::Critical => 3,
        }
    }

    pub fn from_i32(v: i32) -> Result<Self, String> {
        match v {
            0 => Ok(Self::Low),
            1 => Ok(Self::Medium),
            2 => Ok(Self::High),
            3 => Ok(Self::Critical),
            _ => Err(format!("unknown priority value: {v}")),
        }
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

impl FromStr for Priority {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "critical" => Ok(Self::Critical),
            _ => Err(format!("unknown priority: {s}")),
        }
    }
}

// -- Entity Structs --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub project_path: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub kind: String,
    pub session_id: String,
    pub agent_id: Option<String>,
    pub data: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub session_id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub priority: Priority,
    pub worktree_branch: Option<String>,
    pub base_ref: String,
    pub base_commit: String,
    pub merge_target_ref: String,
    pub outcome_kind: Option<String>,
    pub outcome_data: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDep {
    pub task_id: String,
    pub depends_on: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub session_id: String,
    pub parent_agent_id: Option<String>,
    pub task_id: Option<String>,
    pub role: String,
    pub model: String,
    pub mode: String,
    pub status: String,
    pub token_input: i64,
    pub token_output: i64,
    pub created_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mode {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub default_model: Option<String>,
    pub allowed_tools: String,
    pub origin: String,
    pub version: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub id: i64,
    pub trigger_pattern: String,
    pub recommendation: String,
    pub action_type: String,
    pub action_payload: String,
    pub status: String,
    pub applied_at: Option<String>,
    pub reverted_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsRollup {
    pub id: i64,
    pub period_start: String,
    pub period_end: String,
    pub scope: String,
    pub scope_id: Option<String>,
    pub metric: String,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionReport {
    pub events_deleted: usize,
    pub agents_deleted: usize,
    pub tasks_deleted: usize,
    pub sessions_deleted: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UxAgentState {
    pub scope: String,
    pub last_event_id: Option<String>,
    pub last_event_at: Option<String>,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_status_display_roundtrip() {
        let variants = [
            TaskStatus::Pending,
            TaskStatus::InProgress,
            TaskStatus::Blocked,
            TaskStatus::Review,
            TaskStatus::Done,
        ];
        for v in &variants {
            let s = v.to_string();
            let parsed: TaskStatus = s.parse().unwrap();
            assert_eq!(&parsed, v);
        }
    }

    #[test]
    fn task_status_from_str_error() {
        let result: Result<TaskStatus, _> = "invalid".parse();
        assert!(result.is_err());
    }

    #[test]
    fn priority_display_roundtrip() {
        let variants = [
            Priority::Low,
            Priority::Medium,
            Priority::High,
            Priority::Critical,
        ];
        for v in &variants {
            let s = v.to_string();
            let parsed: Priority = s.parse().unwrap();
            assert_eq!(&parsed, v);
        }
    }

    #[test]
    fn priority_i32_roundtrip() {
        let variants = [
            (Priority::Low, 0),
            (Priority::Medium, 1),
            (Priority::High, 2),
            (Priority::Critical, 3),
        ];
        for (p, i) in &variants {
            assert_eq!(p.as_i32(), *i);
            assert_eq!(&Priority::from_i32(*i).unwrap(), p);
        }
    }

    #[test]
    fn priority_from_i32_error() {
        assert!(Priority::from_i32(99).is_err());
    }

    #[test]
    fn session_serde_roundtrip() {
        let session = Session {
            id: "s-1".into(),
            project_path: "/tmp/project".into(),
            created_at: "2026-01-01T00:00:00".into(),
        };
        let json = serde_json::to_string(&session).unwrap();
        let parsed: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, session.id);
        assert_eq!(parsed.project_path, session.project_path);
    }

    #[test]
    fn task_serde_with_enums() {
        let task = Task {
            id: "t-1".into(),
            session_id: "s-1".into(),
            parent_id: None,
            title: "Test task".into(),
            description: Some("desc".into()),
            status: TaskStatus::InProgress,
            priority: Priority::High,
            worktree_branch: Some("kernel/task-test".into()),
            base_ref: "main".into(),
            base_commit: "abc123".into(),
            merge_target_ref: "main".into(),
            outcome_kind: None,
            outcome_data: None,
            created_at: "2026-01-01T00:00:00".into(),
            updated_at: "2026-01-01T00:00:00".into(),
            completed_at: None,
        };
        let json = serde_json::to_string(&task).unwrap();
        let parsed: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, TaskStatus::InProgress);
        assert_eq!(parsed.priority, Priority::High);
    }

    #[test]
    fn event_serde_with_optional_agent() {
        let event = Event {
            id: "e-1".into(),
            kind: "prompt_submitted".into(),
            session_id: "s-1".into(),
            agent_id: None,
            data: r#"{"prompt":"hello"}"#.into(),
            created_at: "2026-01-01T00:00:00".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.agent_id, None);
        assert_eq!(parsed.kind, "prompt_submitted");
    }

    #[test]
    fn agent_serde_roundtrip() {
        let agent = Agent {
            id: "a-1".into(),
            session_id: "s-1".into(),
            parent_agent_id: None,
            task_id: Some("t-1".into()),
            role: "implementation".into(),
            model: "claude-sonnet-4-20250514".into(),
            mode: "implement".into(),
            status: "running".into(),
            token_input: 1000,
            token_output: 500,
            created_at: "2026-01-01T00:00:00".into(),
            finished_at: None,
        };
        let json = serde_json::to_string(&agent).unwrap();
        let parsed: Agent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.token_input, 1000);
        assert_eq!(parsed.task_id, Some("t-1".into()));
    }

    #[test]
    fn mode_serde_roundtrip() {
        let mode = Mode {
            name: "plan".into(),
            description: "Planning mode".into(),
            system_prompt: "You are a planner.".into(),
            default_model: None,
            allowed_tools: r#"["fs_read","git"]"#.into(),
            origin: "builtin".into(),
            version: 1,
            created_at: "2026-01-01T00:00:00".into(),
            updated_at: "2026-01-01T00:00:00".into(),
        };
        let json = serde_json::to_string(&mode).unwrap();
        let parsed: Mode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "plan");
        assert_eq!(parsed.default_model, None);
    }

    #[test]
    fn recommendation_serde_roundtrip() {
        let rec = Recommendation {
            id: 1,
            trigger_pattern: "plan_rejected x3".into(),
            recommendation: "Switch planning model".into(),
            action_type: "model_change".into(),
            action_payload: r#"{"model":"claude-opus-4-20250514"}"#.into(),
            status: "pending".into(),
            applied_at: None,
            reverted_at: None,
            created_at: "2026-01-01T00:00:00".into(),
        };
        let json = serde_json::to_string(&rec).unwrap();
        let parsed: Recommendation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, 1);
        assert_eq!(parsed.status, "pending");
    }

    #[test]
    fn stats_rollup_serde_roundtrip() {
        let rollup = StatsRollup {
            id: 1,
            period_start: "2026-01-01T00:00:00".into(),
            period_end: "2026-01-02T00:00:00".into(),
            scope: "session".into(),
            scope_id: Some("s-1".into()),
            metric: "cost.usd".into(),
            value: 3.50,
        };
        let json = serde_json::to_string(&rollup).unwrap();
        let parsed: StatsRollup = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.value, 3.50);
        assert_eq!(parsed.scope_id, Some("s-1".into()));
    }

    #[test]
    fn ux_agent_state_serde_roundtrip() {
        let state = UxAgentState {
            scope: "global".into(),
            last_event_id: Some("e-42".into()),
            last_event_at: Some("2026-01-01T00:00:00".into()),
            updated_at: "2026-01-01T00:00:00".into(),
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: UxAgentState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.scope, "global");
        assert_eq!(parsed.last_event_id, Some("e-42".into()));
    }
}
