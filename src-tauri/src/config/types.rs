use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KernelConfig {
    pub general: GeneralConfig,
    pub models: ModelsConfig,
    pub branching: BranchingConfig,
    pub compaction: CompactionConfig,
    pub costs: CostsConfig,
    pub retention: RetentionConfig,
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            models: ModelsConfig::default(),
            branching: BranchingConfig::default(),
            compaction: CompactionConfig::default(),
            costs: CostsConfig::default(),
            retention: RetentionConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub engagement: EngagementLevel,
    pub max_concurrent_agents: usize,
    pub worktree_dir: String,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            engagement: EngagementLevel::Collaborative,
            max_concurrent_agents: 4,
            worktree_dir: ".worktrees".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EngagementLevel {
    Autonomous,
    ReviewGates,
    Collaborative,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelsConfig {
    pub default: String,
    pub prompt_router: String,
    pub ux_agent: String,
    pub compactor: String,
    pub roles: RoleModels,
    pub providers: HashMap<String, ProviderConfig>,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            default: "claude-sonnet-4-6".to_string(),
            prompt_router: "claude-haiku-4-5".to_string(),
            ux_agent: "claude-haiku-4-5".to_string(),
            compactor: "claude-haiku-4-5".to_string(),
            roles: RoleModels::default(),
            providers: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleModels {
    pub orchestrator: String,
    pub research: String,
    pub implementation: String,
    pub review: String,
    pub test: String,
    pub unstuck: String,
}

impl Default for RoleModels {
    fn default() -> Self {
        Self {
            orchestrator: "claude-sonnet-4-6".to_string(),
            research: "claude-sonnet-4-6".to_string(),
            implementation: "claude-sonnet-4-6".to_string(),
            review: "claude-sonnet-4-6".to_string(),
            test: "claude-sonnet-4-6".to_string(),
            unstuck: "claude-sonnet-4-6".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub api_key_env: Option<String>,
    pub base_url: Option<String>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            api_key_env: None,
            base_url: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BranchingConfig {
    pub enabled: bool,
    pub max_parallel: usize,
    pub auto_suggest: bool,
}

impl Default for BranchingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_parallel: 3,
            auto_suggest: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactionConfig {
    pub light_every_turn: bool,
    pub deep_trigger_pct: f32,
    pub deep_target_pct: f32,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            light_every_turn: true,
            deep_trigger_pct: 80.0,
            deep_target_pct: 50.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CostsConfig {
    pub warn_at_usd: f64,
    pub hard_limit_usd: f64,
    pub warn_at_task_usd: f64,
    pub hard_limit_task_usd: f64,
}

impl Default for CostsConfig {
    fn default() -> Self {
        Self {
            warn_at_usd: 5.0,
            hard_limit_usd: 20.0,
            warn_at_task_usd: 2.0,
            hard_limit_task_usd: 10.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetentionConfig {
    pub raw_ttl_days: u32,
    pub stats_retention: String,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            raw_ttl_days: 30,
            stats_retention: "forever".to_string(),
        }
    }
}
