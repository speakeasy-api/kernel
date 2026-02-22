use std::path::Path;

use crate::config::KernelConfig;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("failed to parse config file {path}: {source}")]
    Parse {
        path: String,
        source: toml::de::Error,
    },
    #[error("config validation failed: {0}")]
    Validation(String),
}

/// Load config from global (~/.config/kernel/config.toml) and project (kernel.toml) files,
/// merging project over global over defaults at the field level.
pub fn load_config(project_root: &Path) -> Result<KernelConfig, ConfigError> {
    let defaults =
        toml::Value::try_from(&KernelConfig::default()).expect("default config must serialize");

    let global_path = dirs::config_dir().map(|d| d.join("kernel").join("config.toml"));

    let merged = match global_path {
        Some(ref p) if p.exists() => {
            let global = load_toml_value(p)?;
            deep_merge(defaults, global)
        }
        _ => defaults,
    };

    let project_path = project_root.join("kernel.toml");
    let merged = if project_path.exists() {
        let project = load_toml_value(&project_path)?;
        deep_merge(merged, project)
    } else {
        merged
    };

    let config: KernelConfig = merged.try_into().map_err(|e| ConfigError::Parse {
        path: "merged config".to_string(),
        source: e,
    })?;

    validate_config(&config)?;

    Ok(config)
}

/// Validate config values for logical consistency.
pub fn validate_config(config: &KernelConfig) -> Result<(), ConfigError> {
    let mut errors = Vec::new();

    if config.compaction.deep_trigger_pct <= config.compaction.deep_target_pct {
        errors.push(format!(
            "deep_trigger_pct ({}) must be greater than deep_target_pct ({})",
            config.compaction.deep_trigger_pct, config.compaction.deep_target_pct
        ));
    }

    if config.retention.raw_ttl_days == 0 {
        errors.push("raw_ttl_days must be greater than 0".to_string());
    }

    if config.general.max_concurrent_agents == 0 {
        errors.push("max_concurrent_agents must be greater than 0".to_string());
    }

    if config.costs.warn_at_usd >= config.costs.hard_limit_usd {
        errors.push(format!(
            "warn_at_usd ({}) must be less than hard_limit_usd ({})",
            config.costs.warn_at_usd, config.costs.hard_limit_usd
        ));
    }

    if config.costs.warn_at_task_usd >= config.costs.hard_limit_task_usd {
        errors.push(format!(
            "warn_at_task_usd ({}) must be less than hard_limit_task_usd ({})",
            config.costs.warn_at_task_usd, config.costs.hard_limit_task_usd
        ));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::Validation(errors.join("; ")))
    }
}

fn load_toml_value(path: &Path) -> Result<toml::Value, ConfigError> {
    let contents = std::fs::read_to_string(path).map_err(|e| ConfigError::Read {
        path: path.display().to_string(),
        source: e,
    })?;
    contents
        .parse::<toml::Value>()
        .map_err(|e| ConfigError::Parse {
            path: path.display().to_string(),
            source: e,
        })
}

/// Recursively merge `overlay` into `base`. For tables, merge field-by-field.
/// For all other types, overlay wins.
fn deep_merge(base: toml::Value, overlay: toml::Value) -> toml::Value {
    match (base, overlay) {
        (toml::Value::Table(mut base_map), toml::Value::Table(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let merged = match base_map.remove(&key) {
                    Some(base_val) => deep_merge(base_val, overlay_val),
                    None => overlay_val,
                };
                base_map.insert(key, merged);
            }
            toml::Value::Table(base_map)
        }
        (_, overlay) => overlay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn load_defaults_when_no_files_exist() {
        let dir = TempDir::new().unwrap();
        let config = load_config(dir.path()).unwrap();
        let defaults = KernelConfig::default();

        assert_eq!(
            config.general.max_concurrent_agents,
            defaults.general.max_concurrent_agents
        );
        assert_eq!(
            config.compaction.deep_trigger_pct,
            defaults.compaction.deep_trigger_pct
        );
        assert_eq!(
            config.retention.raw_ttl_days,
            defaults.retention.raw_ttl_days
        );
    }

    #[test]
    fn project_config_overrides_defaults() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("kernel.toml"),
            r#"
[general]
max_concurrent_agents = 8

[retention]
raw_ttl_days = 60
"#,
        )
        .unwrap();

        let config = load_config(dir.path()).unwrap();
        assert_eq!(config.general.max_concurrent_agents, 8);
        assert_eq!(config.retention.raw_ttl_days, 60);
        // untouched fields keep defaults
        assert_eq!(config.general.worktree_dir, ".worktrees");
        assert!(config.branching.enabled);
    }

    #[test]
    fn validation_catches_bad_compaction() {
        let config = KernelConfig {
            compaction: crate::config::CompactionConfig {
                deep_trigger_pct: 40.0,
                deep_target_pct: 50.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("deep_trigger_pct"));
    }

    #[test]
    fn validation_catches_zero_ttl() {
        let config = KernelConfig {
            retention: crate::config::RetentionConfig {
                raw_ttl_days: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("raw_ttl_days"));
    }

    #[test]
    fn validation_catches_zero_agents() {
        let config = KernelConfig {
            general: crate::config::GeneralConfig {
                max_concurrent_agents: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("max_concurrent_agents"));
    }

    #[test]
    fn validation_catches_bad_cost_thresholds() {
        let config = KernelConfig {
            costs: crate::config::CostsConfig {
                warn_at_usd: 20.0,
                hard_limit_usd: 10.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("warn_at_usd"));
    }

    #[test]
    fn validation_passes_for_defaults() {
        validate_config(&KernelConfig::default()).unwrap();
    }

    #[test]
    fn deep_merge_overrides_scalars() {
        let base: toml::Value = toml::from_str("[a]\nb = 1\nc = 2").unwrap();
        let overlay: toml::Value = toml::from_str("[a]\nb = 99").unwrap();
        let merged = deep_merge(base, overlay);
        let table = merged.as_table().unwrap();
        let a = table["a"].as_table().unwrap();
        assert_eq!(a["b"].as_integer(), Some(99));
        assert_eq!(a["c"].as_integer(), Some(2));
    }

    #[test]
    fn deep_merge_adds_new_keys() {
        let base: toml::Value = toml::from_str("[a]\nb = 1").unwrap();
        let overlay: toml::Value = toml::from_str("[a]\nd = 4").unwrap();
        let merged = deep_merge(base, overlay);
        let a = merged.as_table().unwrap()["a"].as_table().unwrap();
        assert_eq!(a["b"].as_integer(), Some(1));
        assert_eq!(a["d"].as_integer(), Some(4));
    }

    #[test]
    fn deep_merge_preserves_untouched_fields() {
        let base: toml::Value = toml::from_str(
            r#"
[general]
max_concurrent_agents = 4
worktree_dir = ".worktrees"
engagement = "collaborative"

[retention]
raw_ttl_days = 30
stats_retention = "forever"
"#,
        )
        .unwrap();
        let overlay: toml::Value = toml::from_str(
            r#"
[general]
max_concurrent_agents = 8
"#,
        )
        .unwrap();
        let merged = deep_merge(base, overlay);
        let general = merged.as_table().unwrap()["general"].as_table().unwrap();
        assert_eq!(general["max_concurrent_agents"].as_integer(), Some(8));
        assert_eq!(general["worktree_dir"].as_str(), Some(".worktrees"));
        assert_eq!(general["engagement"].as_str(), Some("collaborative"));
        // entirely untouched section preserved
        let retention = merged.as_table().unwrap()["retention"].as_table().unwrap();
        assert_eq!(retention["raw_ttl_days"].as_integer(), Some(30));
    }

    #[test]
    fn parse_full_kernel_toml() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("kernel.toml"),
            r#"
[general]
engagement = "autonomous"
max_concurrent_agents = 8
worktree_dir = ".wt"

[models]
default = "claude-opus-4-6"
prompt_router = "claude-haiku-4-5"
ux_agent = "claude-haiku-4-5"
compactor = "claude-sonnet-4-6"

[models.roles]
orchestrator = "claude-opus-4-6"
research = "claude-opus-4-6"
implementation = "claude-sonnet-4-6"
review = "claude-opus-4-6"
test = "claude-sonnet-4-6"
unstuck = "claude-opus-4-6"

[models.providers.anthropic]
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com"

[branching]
enabled = false
max_parallel = 5
auto_suggest = false

[compaction]
light_every_turn = false
deep_trigger_pct = 90.0
deep_target_pct = 40.0

[costs]
warn_at_usd = 10.0
hard_limit_usd = 50.0
warn_at_task_usd = 5.0
hard_limit_task_usd = 25.0

[retention]
raw_ttl_days = 90
stats_retention = "forever"
"#,
        )
        .unwrap();

        let config = load_config(dir.path()).unwrap();
        assert_eq!(
            config.general.engagement,
            crate::config::EngagementLevel::Autonomous
        );
        assert_eq!(config.general.max_concurrent_agents, 8);
        assert_eq!(config.general.worktree_dir, ".wt");
        assert_eq!(config.models.default, "claude-opus-4-6");
        assert_eq!(config.models.compactor, "claude-sonnet-4-6");
        assert_eq!(config.models.roles.orchestrator, "claude-opus-4-6");
        assert_eq!(config.models.roles.implementation, "claude-sonnet-4-6");
        assert_eq!(config.models.providers.len(), 1);
        let anthropic = &config.models.providers["anthropic"];
        assert_eq!(anthropic.api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
        assert_eq!(
            anthropic.base_url.as_deref(),
            Some("https://api.anthropic.com")
        );
        assert!(!config.branching.enabled);
        assert_eq!(config.branching.max_parallel, 5);
        assert!(!config.branching.auto_suggest);
        assert!(!config.compaction.light_every_turn);
        assert_eq!(config.compaction.deep_trigger_pct, 90.0);
        assert_eq!(config.compaction.deep_target_pct, 40.0);
        assert_eq!(config.costs.warn_at_usd, 10.0);
        assert_eq!(config.costs.hard_limit_usd, 50.0);
        assert_eq!(config.costs.warn_at_task_usd, 5.0);
        assert_eq!(config.costs.hard_limit_task_usd, 25.0);
        assert_eq!(config.retention.raw_ttl_days, 90);
        assert_eq!(config.retention.stats_retention, "forever");
    }

    #[test]
    fn merge_project_overrides_global_via_deep_merge() {
        // Simulate global + project merge using deep_merge directly
        let global: toml::Value = toml::from_str(
            r#"
[general]
engagement = "review_gates"
max_concurrent_agents = 2

[costs]
warn_at_usd = 3.0
hard_limit_usd = 15.0
warn_at_task_usd = 1.0
hard_limit_task_usd = 5.0

[retention]
raw_ttl_days = 60
"#,
        )
        .unwrap();
        let project: toml::Value = toml::from_str(
            r#"
[general]
engagement = "autonomous"

[costs]
warn_at_usd = 10.0
hard_limit_usd = 50.0
"#,
        )
        .unwrap();

        let defaults = toml::Value::try_from(&KernelConfig::default()).unwrap();
        let merged = deep_merge(deep_merge(defaults, global), project);
        let config: KernelConfig = merged.try_into().unwrap();

        // project overrides global
        assert_eq!(
            config.general.engagement,
            crate::config::EngagementLevel::Autonomous
        );
        assert_eq!(config.costs.warn_at_usd, 10.0);
        assert_eq!(config.costs.hard_limit_usd, 50.0);
        // global values preserved where project doesn't override
        assert_eq!(config.general.max_concurrent_agents, 2);
        assert_eq!(config.costs.warn_at_task_usd, 1.0);
        assert_eq!(config.costs.hard_limit_task_usd, 5.0);
        assert_eq!(config.retention.raw_ttl_days, 60);
        // defaults preserved where neither global nor project set
        assert_eq!(config.general.worktree_dir, ".worktrees");
        assert!(config.branching.enabled);
    }

    #[test]
    fn validation_catches_bad_task_cost_thresholds() {
        let config = KernelConfig {
            costs: crate::config::CostsConfig {
                warn_at_task_usd: 15.0,
                hard_limit_task_usd: 5.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("warn_at_task_usd"));
    }

    #[test]
    fn engagement_level_deserializes_from_lowercase() {
        #[derive(Deserialize)]
        struct Wrapper {
            engagement: crate::config::EngagementLevel,
        }

        for (input, expected) in [
            ("autonomous", crate::config::EngagementLevel::Autonomous),
            ("review_gates", crate::config::EngagementLevel::ReviewGates),
            (
                "collaborative",
                crate::config::EngagementLevel::Collaborative,
            ),
        ] {
            let toml_str = format!("engagement = \"{}\"", input);
            let w: Wrapper = toml::from_str(&toml_str).unwrap();
            assert_eq!(w.engagement, expected, "failed for input: {}", input);
        }
    }
}
