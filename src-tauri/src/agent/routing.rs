use std::collections::HashMap;

use super::orchestrator::RoleModelDefaults;
use super::types::AgentRole;

/// Configuration for model routing of child agents.
/// Loaded from kernel.toml `models.roles.*` section.
#[derive(Debug, Clone)]
pub struct ModelRoutingConfig {
    pub role_defaults: HashMap<AgentRole, String>,
    pub fallback_model: String,
}

impl ModelRoutingConfig {
    /// Create model routing config from role defaults and fallback model name.
    pub fn from_role_defaults(defaults: &RoleModelDefaults, fallback: String) -> Self {
        let mut map = HashMap::new();
        map.insert(AgentRole::Orchestrator, defaults.orchestrator.clone());
        map.insert(AgentRole::Research, defaults.research.clone());
        map.insert(AgentRole::Implementation, defaults.implementation.clone());
        map.insert(AgentRole::Review, defaults.review.clone());
        map.insert(AgentRole::Test, defaults.test.clone());
        map.insert(AgentRole::Unstuck, defaults.unstuck.clone());
        Self {
            role_defaults: map,
            fallback_model: fallback,
        }
    }
}

/// Determine the model for a child agent.
///
/// Priority:
/// 1. Explicit override in spawn request
/// 2. Role default from config
/// 3. Fallback model
pub fn route_model(
    config: &ModelRoutingConfig,
    role: &AgentRole,
    explicit_override: Option<&str>,
) -> String {
    if let Some(model) = explicit_override {
        return model.to_string();
    }

    config
        .role_defaults
        .get(role)
        .cloned()
        .unwrap_or_else(|| config.fallback_model.clone())
}

/// Suggest the default mode for a given agent role.
pub fn default_mode_for_role(role: &AgentRole) -> &'static str {
    match role {
        AgentRole::Orchestrator => "plan",
        AgentRole::Research => "research",
        AgentRole::Implementation => "implement",
        AgentRole::Test => "implement",
        AgentRole::Review => "review",
        AgentRole::Unstuck => "debug",
    }
}

/// Default tool permissions for a given agent role.
pub fn default_tools_for_role(role: &AgentRole) -> Vec<String> {
    match role {
        AgentRole::Orchestrator => vec![],
        AgentRole::Research => vec!["fs_read", "glob", "grep", "web_search", "web_fetch"]
            .into_iter()
            .map(String::from)
            .collect(),
        AgentRole::Implementation | AgentRole::Test | AgentRole::Unstuck => {
            vec!["fs_read", "fs_write", "glob", "grep", "shell", "git"]
                .into_iter()
                .map(String::from)
                .collect()
        }
        AgentRole::Review => vec!["fs_read", "glob", "grep", "git"]
            .into_iter()
            .map(String::from)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role_defaults() -> RoleModelDefaults {
        RoleModelDefaults {
            orchestrator: "orchestrator-model".to_string(),
            research: "research-model".to_string(),
            implementation: "implementation-model".to_string(),
            review: "review-model".to_string(),
            test: "test-model".to_string(),
            unstuck: "unstuck-model".to_string(),
        }
    }

    #[test]
    fn route_model_uses_override_first() {
        let config =
            ModelRoutingConfig::from_role_defaults(&role_defaults(), "fallback-model".to_string());
        let model = route_model(&config, &AgentRole::Research, Some("override-model"));

        assert_eq!(model, "override-model");
    }

    #[test]
    fn route_model_uses_role_default_when_no_override() {
        let config =
            ModelRoutingConfig::from_role_defaults(&role_defaults(), "fallback-model".to_string());
        let model = route_model(&config, &AgentRole::Implementation, None);

        assert_eq!(model, "implementation-model");
    }

    #[test]
    fn route_model_uses_fallback_when_role_default_missing() {
        let mut config =
            ModelRoutingConfig::from_role_defaults(&role_defaults(), "fallback-model".to_string());
        config.role_defaults.remove(&AgentRole::Test);

        let model = route_model(&config, &AgentRole::Test, None);

        assert_eq!(model, "fallback-model");
    }

    #[test]
    fn default_modes_match_role_expectations() {
        assert_eq!(default_mode_for_role(&AgentRole::Orchestrator), "plan");
        assert_eq!(default_mode_for_role(&AgentRole::Research), "research");
        assert_eq!(
            default_mode_for_role(&AgentRole::Implementation),
            "implement"
        );
        assert_eq!(default_mode_for_role(&AgentRole::Test), "implement");
        assert_eq!(default_mode_for_role(&AgentRole::Review), "review");
        assert_eq!(default_mode_for_role(&AgentRole::Unstuck), "debug");
    }

    #[test]
    fn default_tools_match_role_permissions() {
        assert!(default_tools_for_role(&AgentRole::Orchestrator).is_empty());
        assert_eq!(
            default_tools_for_role(&AgentRole::Research),
            vec!["fs_read", "glob", "grep", "web_search", "web_fetch"]
        );
        assert_eq!(
            default_tools_for_role(&AgentRole::Implementation),
            vec!["fs_read", "fs_write", "glob", "grep", "shell", "git"]
        );
        assert_eq!(
            default_tools_for_role(&AgentRole::Test),
            vec!["fs_read", "fs_write", "glob", "grep", "shell", "git"]
        );
        assert_eq!(
            default_tools_for_role(&AgentRole::Review),
            vec!["fs_read", "glob", "grep", "git"]
        );
        assert_eq!(
            default_tools_for_role(&AgentRole::Unstuck),
            vec!["fs_read", "fs_write", "glob", "grep", "shell", "git"]
        );
    }

    #[test]
    fn from_role_defaults_populates_all_roles() {
        let defaults = role_defaults();
        let config =
            ModelRoutingConfig::from_role_defaults(&defaults, "fallback-model".to_string());

        assert_eq!(
            config.role_defaults.get(&AgentRole::Orchestrator),
            Some(&defaults.orchestrator)
        );
        assert_eq!(
            config.role_defaults.get(&AgentRole::Research),
            Some(&defaults.research)
        );
        assert_eq!(
            config.role_defaults.get(&AgentRole::Implementation),
            Some(&defaults.implementation)
        );
        assert_eq!(
            config.role_defaults.get(&AgentRole::Review),
            Some(&defaults.review)
        );
        assert_eq!(
            config.role_defaults.get(&AgentRole::Test),
            Some(&defaults.test)
        );
        assert_eq!(
            config.role_defaults.get(&AgentRole::Unstuck),
            Some(&defaults.unstuck)
        );
    }
}
