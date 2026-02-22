use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recommendation {
    pub id: u64,
    pub trigger_pattern: String,
    pub recommendation: String,
    pub action: RecommendationAction,
    pub status: RecommendationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RecommendationAction {
    ModelChange {
        role: String,
        from_model: String,
        to_model: String,
    },
    PromptEdit {
        mode_name: String,
        old_fragment: String,
        new_fragment: String,
    },
    ModeCreate {
        name: String,
        description: String,
        system_prompt: String,
        default_model: Option<String>,
        allowed_tools: Vec<String>,
    },
    ModeEdit {
        mode_name: String,
        changes: ModeChanges,
    },
    ConfigChange {
        key: String,
        old_value: String,
        new_value: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeChanges {
    pub description: Option<String>,
    pub system_prompt: Option<String>,
    pub default_model: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecommendationStatus {
    Pending,
    Applied,
    Dismissed,
    Reverted,
}

impl fmt::Display for RecommendationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Pending => "Pending",
            Self::Applied => "Applied",
            Self::Dismissed => "Dismissed",
            Self::Reverted => "Reverted",
        };
        write!(f, "{value}")
    }
}

impl FromStr for RecommendationStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" | "pending" => Ok(Self::Pending),
            "Applied" | "applied" => Ok(Self::Applied),
            "Dismissed" | "dismissed" => Ok(Self::Dismissed),
            "Reverted" | "reverted" => Ok(Self::Reverted),
            _ => Err(format!("unknown recommendation status: {s}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecommendationVersion {
    pub id: u64,
    pub recommendation_id: u64,
    pub version: u32,
    pub applied_at: String,
    pub reverted_at: Option<String>,
    pub snapshot: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UxAgentState {
    pub last_event_id: Option<String>,
    pub last_event_at: Option<String>,
    pub last_run_at: Option<String>,
}
