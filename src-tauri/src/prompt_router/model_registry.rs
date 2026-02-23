use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Well-known stable model used when the router picks an unknown ID.
pub const FALLBACK_MODEL: &str = "anthropic/claude-sonnet-4-6";

/// Maximum models stored per category to keep the prompt compact.
const MAX_MODELS_PER_CATEGORY: usize = 6;

/// Minimal model metadata surfaced in the router prompt.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub context_length: u64,
}

/// Shape returned by `GET https://openrouter.ai/api/v1/models?category=…`
#[derive(Debug, Deserialize)]
struct OpenRouterModelsResponse {
    data: Vec<OpenRouterModel>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterModel {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    context_length: u64,
}

/// Return the OpenRouter category slugs relevant to a given mode name.
pub fn categories_for_mode(mode: &str) -> &'static [&'static str] {
    match mode.to_lowercase().as_str() {
        "plan" => &["programming", "technology"],
        "implement" => &["programming"],
        "review" => &["programming"],
        "debug" => &["programming", "technology"],
        "research" => &["science", "technology", "academia"],
        "general" => &["programming", "technology"],
        _ => &["programming", "technology"],
    }
}

/// Cached registry of models fetched from OpenRouter, keyed by category.
pub struct ModelRegistry {
    client: reqwest::Client,
    cache: RwLock<HashMap<String, Vec<ModelInfo>>>,
}

impl ModelRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            client: reqwest::Client::new(),
            cache: RwLock::new(HashMap::new()),
        })
    }

    /// Fetch models for every known category from OpenRouter and update cache.
    /// Logs failures but keeps stale data — callers never see a hard error.
    pub async fn refresh(&self) {
        let all_categories: &[&str] = &[
            "programming",
            "technology",
            "science",
            "academia",
        ];

        for &cat in all_categories {
            match self.fetch_category(cat).await {
                Ok(models) => {
                    let mut cache = self.cache.write().await;
                    cache.insert(cat.to_string(), models);
                }
                Err(e) => {
                    tracing::warn!(category = cat, error = %e, "failed to refresh model registry");
                }
            }
        }
    }

    async fn fetch_category(&self, category: &str) -> Result<Vec<ModelInfo>, String> {
        let url = format!(
            "https://openrouter.ai/api/v1/models?category={category}"
        );

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        let body: OpenRouterModelsResponse =
            resp.json().await.map_err(|e| e.to_string())?;

        let mut models: Vec<ModelInfo> = body
            .data
            .into_iter()
            .map(|m| ModelInfo {
                id: m.id,
                name: m.name,
                description: m.description,
                context_length: m.context_length,
            })
            .collect();

        // Sort by context_length descending, take top N
        models.sort_by(|a, b| b.context_length.cmp(&a.context_length));
        models.truncate(MAX_MODELS_PER_CATEGORY);

        Ok(models)
    }

    /// Return deduplicated models relevant to a mode. Uses `try_read()` so
    /// callers in `spawn_blocking` never block on a concurrent refresh.
    /// Returns an empty vec on lock contention or cold cache.
    pub fn models_for_mode(&self, mode: &str) -> Vec<ModelInfo> {
        let guard = match self.cache.try_read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };

        let cats = categories_for_mode(mode);
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();

        for cat in cats {
            if let Some(models) = guard.get(*cat) {
                for m in models {
                    if seen.insert(m.id.clone()) {
                        out.push(m.clone());
                    }
                }
            }
        }
        out
    }

    /// Check if any cached category contains the given model ID.
    pub fn is_model_known(&self, model_id: &str) -> bool {
        let guard = match self.cache.try_read() {
            Ok(g) => g,
            Err(_) => return false,
        };

        guard
            .values()
            .any(|models| models.iter().any(|m| m.id == model_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_for_known_modes() {
        assert_eq!(categories_for_mode("plan"), &["programming", "technology"]);
        assert_eq!(categories_for_mode("implement"), &["programming"]);
        assert_eq!(categories_for_mode("review"), &["programming"]);
        assert_eq!(
            categories_for_mode("debug"),
            &["programming", "technology"]
        );
        assert_eq!(
            categories_for_mode("research"),
            &["science", "technology", "academia"]
        );
        assert_eq!(
            categories_for_mode("general"),
            &["programming", "technology"]
        );
    }

    #[test]
    fn categories_for_unknown_mode_returns_default() {
        assert_eq!(
            categories_for_mode("banana"),
            &["programming", "technology"]
        );
    }

    #[test]
    fn models_for_mode_empty_on_cold_cache() {
        let registry = ModelRegistry::new();
        let models = registry.models_for_mode("plan");
        assert!(models.is_empty());
    }

    #[test]
    fn is_model_known_false_on_cold_cache() {
        let registry = ModelRegistry::new();
        assert!(!registry.is_model_known("anything"));
    }

    #[tokio::test]
    async fn models_for_mode_deduplicates() {
        let registry = ModelRegistry::new();
        {
            let mut cache = registry.cache.write().await;
            let model = ModelInfo {
                id: "openai/gpt-4".into(),
                name: "GPT-4".into(),
                description: "".into(),
                context_length: 128_000,
            };
            cache.insert(
                "programming".into(),
                vec![model.clone()],
            );
            cache.insert(
                "technology".into(),
                vec![model],
            );
        }

        // "plan" merges programming + technology — should deduplicate
        let models = registry.models_for_mode("plan");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "openai/gpt-4");
    }

    #[tokio::test]
    async fn is_model_known_finds_cached_model() {
        let registry = ModelRegistry::new();
        {
            let mut cache = registry.cache.write().await;
            cache.insert(
                "programming".into(),
                vec![ModelInfo {
                    id: "anthropic/claude-sonnet-4-6".into(),
                    name: "Claude Sonnet".into(),
                    description: "".into(),
                    context_length: 200_000,
                }],
            );
        }

        assert!(registry.is_model_known("anthropic/claude-sonnet-4-6"));
        assert!(!registry.is_model_known("nonexistent/model"));
    }
}
