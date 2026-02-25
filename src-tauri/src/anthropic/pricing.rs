use tracing::debug;

use super::types::Usage;

#[derive(Debug, Clone, Copy)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_write_per_mtok: f64,
    pub cache_read_per_mtok: f64,
}

/// Strip provider prefixes, date suffixes, and map aliases to canonical names.
pub fn normalize_model_name(model: &str) -> String {
    // Strip provider prefix (e.g. "anthropic/")
    let name = model
        .strip_prefix("anthropic/")
        .unwrap_or(model);

    // Strip date suffix (e.g. "-20250514")
    let name = strip_date_suffix(name);

    // Map version aliases: claude-sonnet-4-6 -> claude-sonnet-4, etc.
    match name {
        "claude-opus-4-6" => "claude-opus-4".into(),
        "claude-sonnet-4-6" => "claude-sonnet-4".into(),
        "claude-haiku-4-5" => "claude-haiku-4-5".into(),
        other => other.to_string(),
    }
}

fn strip_date_suffix(name: &str) -> &str {
    // Date suffixes look like -YYYYMMDD (8 digits after a dash)
    if let Some(pos) = name.rfind('-') {
        let suffix = &name[pos + 1..];
        if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_digit()) {
            return &name[..pos];
        }
    }
    name
}

/// Look up pricing for a normalized model name.
pub fn pricing_for_model(model: &str) -> Option<ModelPricing> {
    let normalized = normalize_model_name(model);
    match normalized.as_str() {
        "claude-opus-4" => Some(ModelPricing {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
            cache_write_per_mtok: 18.75,
            cache_read_per_mtok: 1.50,
        }),
        "claude-sonnet-4" => Some(ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cache_write_per_mtok: 3.75,
            cache_read_per_mtok: 0.30,
        }),
        "claude-haiku-4-5" => Some(ModelPricing {
            input_per_mtok: 0.80,
            output_per_mtok: 4.0,
            cache_write_per_mtok: 1.0,
            cache_read_per_mtok: 0.08,
        }),
        _ => None,
    }
}

/// Calculate cost in USD for a given model and usage.
/// Returns 0.0 for unknown models.
pub fn calculate_cost(model: &str, usage: &Usage) -> f64 {
    let Some(pricing) = pricing_for_model(model) else {
        debug!(model = %model, "unknown model for pricing, returning zero cost");
        return 0.0;
    };

    let input_cost = usage.input_tokens as f64 * pricing.input_per_mtok / 1_000_000.0;
    let output_cost = usage.output_tokens as f64 * pricing.output_per_mtok / 1_000_000.0;
    let cache_write_cost =
        usage.cache_creation_input_tokens as f64 * pricing.cache_write_per_mtok / 1_000_000.0;
    let cache_read_cost =
        usage.cache_read_input_tokens as f64 * pricing.cache_read_per_mtok / 1_000_000.0;

    let total = input_cost + output_cost + cache_write_cost + cache_read_cost;
    debug!(
        model = %model,
        input_tokens = usage.input_tokens,
        output_tokens = usage.output_tokens,
        cache_write_tokens = usage.cache_creation_input_tokens,
        cache_read_tokens = usage.cache_read_input_tokens,
        cost_usd = total,
        "calculated cost"
    );
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_anthropic_prefix() {
        assert_eq!(
            normalize_model_name("anthropic/claude-sonnet-4-20250514"),
            "claude-sonnet-4"
        );
    }

    #[test]
    fn normalize_strips_date_suffix() {
        assert_eq!(
            normalize_model_name("claude-opus-4-20250514"),
            "claude-opus-4"
        );
    }

    #[test]
    fn normalize_maps_version_aliases() {
        assert_eq!(normalize_model_name("claude-sonnet-4-6"), "claude-sonnet-4");
        assert_eq!(normalize_model_name("claude-opus-4-6"), "claude-opus-4");
    }

    #[test]
    fn normalize_haiku() {
        assert_eq!(
            normalize_model_name("claude-haiku-4-5-20251001"),
            "claude-haiku-4-5"
        );
        assert_eq!(
            normalize_model_name("anthropic/claude-haiku-4-5-20251001"),
            "claude-haiku-4-5"
        );
    }

    #[test]
    fn normalize_passthrough_unknown() {
        assert_eq!(normalize_model_name("gpt-4o"), "gpt-4o");
    }

    #[test]
    fn pricing_known_models() {
        assert!(pricing_for_model("claude-opus-4-20250514").is_some());
        assert!(pricing_for_model("claude-sonnet-4-6").is_some());
        assert!(pricing_for_model("claude-haiku-4-5-20251001").is_some());
    }

    #[test]
    fn pricing_unknown_model() {
        assert!(pricing_for_model("gpt-4o").is_none());
    }

    #[test]
    fn calculate_cost_sonnet() {
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let cost = calculate_cost("claude-sonnet-4-20250514", &usage);
        // 1M * 3.0/1M + 100K * 15.0/1M = 3.0 + 1.5 = 4.5
        assert!((cost - 4.5).abs() < 0.001);
    }

    #[test]
    fn calculate_cost_with_cache() {
        let usage = Usage {
            input_tokens: 500_000,
            output_tokens: 50_000,
            cache_creation_input_tokens: 200_000,
            cache_read_input_tokens: 300_000,
        };
        let cost = calculate_cost("claude-sonnet-4-6", &usage);
        // 500K * 3.0/1M + 50K * 15.0/1M + 200K * 3.75/1M + 300K * 0.30/1M
        // = 1.5 + 0.75 + 0.75 + 0.09 = 3.09
        assert!((cost - 3.09).abs() < 0.001);
    }

    #[test]
    fn calculate_cost_unknown_model_returns_zero() {
        let usage = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        };
        assert_eq!(calculate_cost("unknown-model", &usage), 0.0);
    }

    #[test]
    fn calculate_cost_zero_usage() {
        let usage = Usage::default();
        assert_eq!(calculate_cost("claude-opus-4-6", &usage), 0.0);
    }
}
