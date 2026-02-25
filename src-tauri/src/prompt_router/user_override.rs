use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use super::types::{ModeInfo, RouterOutput};

/// Represents a user's manual mode selection that overrides classification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeOverride {
    /// The mode the router originally selected (or would have selected).
    pub original_mode: String,
    /// The mode the user is overriding to.
    pub override_mode: String,
    /// Optional model override to pair with the selected mode.
    pub override_model: Option<String>,
}

/// Event payload for a user override decision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModeOverriddenEvent {
    pub from_mode: String,
    pub to_mode: String,
}

#[derive(Debug)]
pub struct OverrideError {
    pub message: String,
}

impl std::fmt::Display for OverrideError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Override error: {}", self.message)
    }
}

impl std::error::Error for OverrideError {}

/// Apply a user-selected mode override to a router output and build event data.
///
/// Returns a new `RouterOutput` with:
/// - `mode` set to `override_mode`
/// - `model` set to `override_model` when provided, otherwise preserved from `original`
/// - `confidence` set to `1.0` because user choice is explicit
#[instrument(skip(original, available_modes))]
pub fn apply_override(
    original: &RouterOutput,
    override_mode: &str,
    override_model: Option<&str>,
    available_modes: &[ModeInfo],
) -> Result<(RouterOutput, ModeOverriddenEvent), OverrideError> {
    let mode_exists = available_modes
        .iter()
        .any(|mode| mode.name == override_mode);
    if !mode_exists {
        warn!(override_mode, "user override rejected: unknown mode");
        return Err(OverrideError {
            message: format!("mode '{override_mode}' is not in available modes"),
        });
    }

    info!(
        from_mode = %original.mode,
        to_mode = override_mode,
        override_model = ?override_model,
        "user override detected"
    );

    let output = RouterOutput {
        mode: override_mode.to_string(),
        model: override_model.unwrap_or(&original.model).to_string(),
        confidence: 1.0,
    };

    let event = ModeOverriddenEvent {
        from_mode: original.mode.clone(),
        to_mode: override_mode.to_string(),
    };

    debug!(mode = %output.mode, model = %output.model, "override applied");
    Ok((output, event))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes() -> Vec<ModeInfo> {
        vec![
            ModeInfo {
                name: "code".to_string(),
                description: "Code-focused mode".to_string(),
            },
            ModeInfo {
                name: "research".to_string(),
                description: "Research-focused mode".to_string(),
            },
        ]
    }

    fn original_output() -> RouterOutput {
        RouterOutput {
            mode: "code".to_string(),
            model: "gpt-4.1".to_string(),
            confidence: 0.73,
        }
    }

    #[test]
    fn apply_override_uses_override_mode_and_confidence_one() {
        let original = original_output();

        let (overridden, event) =
            apply_override(&original, "research", None, &modes()).expect("valid override");

        assert_eq!(overridden.mode, "research");
        assert_eq!(overridden.confidence, 1.0);
        assert_eq!(
            event,
            ModeOverriddenEvent {
                from_mode: "code".to_string(),
                to_mode: "research".to_string()
            }
        );
    }

    #[test]
    fn apply_override_preserves_model_when_override_model_is_none() {
        let original = original_output();

        let (overridden, _) =
            apply_override(&original, "research", None, &modes()).expect("valid override");

        assert_eq!(overridden.model, "gpt-4.1");
    }

    #[test]
    fn apply_override_uses_override_model_when_provided() {
        let original = original_output();

        let (overridden, _) = apply_override(&original, "research", Some("gpt-4.1-mini"), &modes())
            .expect("valid override");

        assert_eq!(overridden.model, "gpt-4.1-mini");
    }

    #[test]
    fn apply_override_rejects_unknown_mode() {
        let original = original_output();

        let err = apply_override(&original, "unknown", None, &modes()).expect_err("invalid mode");

        assert!(err.to_string().contains("Override error: mode 'unknown'"));
    }
}
