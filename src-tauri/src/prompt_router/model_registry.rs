use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, instrument, warn};

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

/// Build the set of catalog keys to probe for a user-supplied model ID.
/// Tries the canonical form as-is first, then falls back to an
/// `anthropic/`-prefixed variant for bare names like `claude-sonnet-4-6`
/// that the kernel's config uses without a provider prefix.
fn lookup_candidates(model_id: &str) -> Vec<String> {
    let canonical = canonical_model_id(model_id);
    if canonical.contains('/') {
        vec![canonical]
    } else {
        vec![canonical.clone(), format!("anthropic/{canonical}")]
    }
}

/// Canonicalize a model ID so dot- and dash-separated version variants
/// compare equal. OpenRouter uses `anthropic/claude-sonnet-4.6`; configs
/// and fallbacks across the kernel use `anthropic/claude-sonnet-4-6`.
/// Both collapse to the dash form here.
pub fn canonical_model_id(id: &str) -> String {
    let bytes = id.as_bytes();
    let mut out = String::with_capacity(id.len());
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'.'
            && i > 0
            && i + 1 < bytes.len()
            && bytes[i - 1].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
        {
            out.push('-');
        } else {
            out.push(*b as char);
        }
    }
    out
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

/// Cached registry of models fetched from OpenRouter.
///
/// Two-level structure:
/// - **catalog**: full model metadata keyed by canonical model ID
///   (see [`canonical_model_id`]). `ModelInfo.id` retains the original
///   OpenRouter ID so callers can dispatch to the provider with the
///   form it expects.
/// - **categories**: category name → top N canonical model IDs
///   (populated from per-category fetches)
pub struct ModelRegistry {
    client: reqwest::Client,
    /// Full model catalog keyed by model ID (e.g. "anthropic/claude-sonnet-4-6")
    catalog: RwLock<HashMap<String, ModelInfo>>,
    /// Category index: category name → model IDs (top N per category)
    categories: RwLock<HashMap<String, Vec<String>>>,
}

impl ModelRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            client: reqwest::Client::new(),
            catalog: RwLock::new(HashMap::new()),
            categories: RwLock::new(HashMap::new()),
        })
    }

    /// Fetch all models and per-category indices from OpenRouter and update caches.
    /// Logs failures but keeps stale data — callers never see a hard error.
    #[instrument(skip(self))]
    pub async fn refresh(&self) {
        info!("refreshing model catalog");
        // Step 1: Fetch full catalog (unfiltered)
        match self.fetch_all_models().await {
            Ok(models) => {
                debug!(model_count = models.len(), "fetched model catalog");
                let mut catalog = self.catalog.write().await;
                catalog.clear();
                for m in models {
                    catalog.insert(canonical_model_id(&m.id), m);
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to refresh model catalog");
            }
        }

        // Step 2: Fetch per-category indices
        let all_categories: &[&str] = &["programming", "technology", "science", "academia"];

        for &cat in all_categories {
            match self.fetch_category_ids(cat).await {
                Ok(ids) => {
                    debug!(
                        category = cat,
                        model_count = ids.len(),
                        "fetched category index"
                    );
                    let canonical_ids = ids
                        .into_iter()
                        .map(|id| canonical_model_id(&id))
                        .collect();
                    let mut categories = self.categories.write().await;
                    categories.insert(cat.to_string(), canonical_ids);
                }
                Err(e) => {
                    warn!(category = cat, error = %e, "failed to refresh category index");
                }
            }
        }
        info!("model catalog refresh complete");
    }

    /// Fetch all models from the unfiltered endpoint.
    async fn fetch_all_models(&self) -> Result<Vec<ModelInfo>, String> {
        let url = "https://openrouter.ai/api/v1/models";

        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        let body: OpenRouterModelsResponse = resp.json().await.map_err(|e| e.to_string())?;

        Ok(body
            .data
            .into_iter()
            .map(|m| ModelInfo {
                id: m.id,
                name: m.name,
                description: m.description,
                context_length: m.context_length,
            })
            .collect())
    }

    /// Fetch top N model IDs for a category.
    async fn fetch_category_ids(&self, category: &str) -> Result<Vec<String>, String> {
        let url = format!("https://openrouter.ai/api/v1/models?category={category}");

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        let body: OpenRouterModelsResponse = resp.json().await.map_err(|e| e.to_string())?;

        let mut models: Vec<(String, u64)> = body
            .data
            .into_iter()
            .map(|m| (m.id, m.context_length))
            .collect();

        // Sort by context_length descending, take top N
        models.sort_by(|a, b| b.1.cmp(&a.1));
        models.truncate(MAX_MODELS_PER_CATEGORY);

        Ok(models.into_iter().map(|(id, _)| id).collect())
    }

    /// Look up context_length for a model ID. Accepts any variant of the
    /// ID (dash/dot, with or without provider prefix) — matches by
    /// canonical form.
    pub fn context_length_for_model(&self, model_id: &str) -> Option<u64> {
        let guard = self.catalog.try_read().ok()?;
        for candidate in lookup_candidates(model_id) {
            if let Some(m) = guard.get(&candidate) {
                return Some(m.context_length);
            }
        }
        None
    }

    /// Resolve a user-supplied model ID (either dot or dash form, with or
    /// without provider prefix) to the canonical OpenRouter ID stored in
    /// the catalog. Use this before calling the provider so renames like
    /// `4-6` → `4.6` don't break requests. Returns `None` if the model is
    /// not in the catalog.
    pub fn resolve_catalog_id(&self, model_id: &str) -> Option<String> {
        let guard = self.catalog.try_read().ok()?;
        for candidate in lookup_candidates(model_id) {
            if let Some(m) = guard.get(&candidate) {
                return Some(m.id.clone());
            }
        }
        None
    }

    /// Ensure the registry has been populated at least once. Call this before
    /// routing so the classifier has a model list to work with.
    pub async fn ensure_warm(&self) {
        let is_cold = {
            let cat = self.catalog.read().await;
            cat.is_empty()
        };
        if is_cold {
            self.refresh().await;
        }
    }

    /// Return deduplicated models relevant to a mode.
    #[instrument(skip(self))]
    pub async fn models_for_mode(&self, mode: &str) -> Vec<ModelInfo> {
        let cats = categories_for_mode(mode);

        // Collect category IDs first, release lock before next await
        let all_ids: Vec<String> = {
            let cat_guard = self.categories.read().await;
            let mut seen = HashSet::new();
            let mut ids = Vec::new();
            for cat in cats {
                if let Some(cat_ids) = cat_guard.get(*cat) {
                    for id in cat_ids {
                        if seen.insert(id.clone()) {
                            ids.push(id.clone());
                        }
                    }
                }
            }
            ids
        };

        // Look up full info from catalog
        let catalog_guard = self.catalog.read().await;
        let out: Vec<ModelInfo> = all_ids
            .iter()
            .filter_map(|id| catalog_guard.get(id).cloned())
            .collect();

        debug!(mode, model_count = out.len(), "resolved models for mode");
        out
    }

    /// Return all model IDs from the full catalog (for validation).
    pub async fn catalog_ids(&self) -> HashSet<String> {
        let catalog = self.catalog.read().await;
        catalog.keys().cloned().collect()
    }

    /// Check if any cached category contains the given model ID. Matches
    /// by canonical form so dot/dash variants both succeed.
    pub fn is_model_known(&self, model_id: &str) -> bool {
        let guard = match self.categories.try_read() {
            Ok(g) => g,
            Err(_) => return false,
        };

        let candidates = lookup_candidates(model_id);
        guard
            .values()
            .any(|ids| ids.iter().any(|id| candidates.iter().any(|c| c == id)))
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
        assert_eq!(categories_for_mode("debug"), &["programming", "technology"]);
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
    fn canonical_id_collapses_digit_dot_to_dash() {
        assert_eq!(
            canonical_model_id("anthropic/claude-sonnet-4.6"),
            "anthropic/claude-sonnet-4-6"
        );
        assert_eq!(
            canonical_model_id("anthropic/claude-sonnet-4-6"),
            "anthropic/claude-sonnet-4-6"
        );
        assert_eq!(
            canonical_model_id("anthropic/claude-opus-4.6-fast"),
            "anthropic/claude-opus-4-6-fast"
        );
        // Non-numeric dots (e.g. domain-style) preserved.
        assert_eq!(canonical_model_id("foo.bar/baz"), "foo.bar/baz");
        // Trailing dot preserved.
        assert_eq!(canonical_model_id("model-4."), "model-4.");
    }

    #[tokio::test]
    async fn context_length_matches_dot_and_dash_variants() {
        let registry = ModelRegistry::new();
        {
            let mut catalog = registry.catalog.write().await;
            catalog.insert(
                canonical_model_id("anthropic/claude-sonnet-4.6"),
                ModelInfo {
                    id: "anthropic/claude-sonnet-4.6".into(),
                    name: "Claude Sonnet 4.6".into(),
                    description: "".into(),
                    context_length: 1_000_000,
                },
            );
        }
        assert_eq!(
            registry.context_length_for_model("anthropic/claude-sonnet-4-6"),
            Some(1_000_000)
        );
        assert_eq!(
            registry.context_length_for_model("anthropic/claude-sonnet-4.6"),
            Some(1_000_000)
        );
        // Unprefixed kernel config default also resolves.
        assert_eq!(
            registry.context_length_for_model("claude-sonnet-4-6"),
            Some(1_000_000)
        );
    }

    #[tokio::test]
    async fn resolve_catalog_id_returns_original_provider_form() {
        let registry = ModelRegistry::new();
        {
            let mut catalog = registry.catalog.write().await;
            catalog.insert(
                canonical_model_id("anthropic/claude-sonnet-4.6"),
                ModelInfo {
                    id: "anthropic/claude-sonnet-4.6".into(),
                    name: "Claude Sonnet 4.6".into(),
                    description: "".into(),
                    context_length: 1_000_000,
                },
            );
        }
        assert_eq!(
            registry.resolve_catalog_id("anthropic/claude-sonnet-4-6"),
            Some("anthropic/claude-sonnet-4.6".into())
        );
        assert_eq!(
            registry.resolve_catalog_id("claude-sonnet-4-6"),
            Some("anthropic/claude-sonnet-4.6".into())
        );
        assert_eq!(registry.resolve_catalog_id("unknown/model"), None);
    }

    #[tokio::test]
    async fn models_for_mode_empty_on_cold_cache() {
        let registry = ModelRegistry::new();
        let models = registry.models_for_mode("plan").await;
        assert!(models.is_empty());
    }

    #[test]
    fn is_model_known_false_on_cold_cache() {
        let registry = ModelRegistry::new();
        assert!(!registry.is_model_known("anything"));
    }

    #[test]
    fn context_length_for_model_none_on_cold_cache() {
        let registry = ModelRegistry::new();
        assert!(registry.context_length_for_model("anything").is_none());
    }

    #[tokio::test]
    async fn context_length_for_model_returns_value() {
        let registry = ModelRegistry::new();
        {
            let mut catalog = registry.catalog.write().await;
            catalog.insert(
                "anthropic/claude-sonnet-4-6".into(),
                ModelInfo {
                    id: "anthropic/claude-sonnet-4-6".into(),
                    name: "Claude Sonnet".into(),
                    description: "".into(),
                    context_length: 200_000,
                },
            );
        }
        assert_eq!(
            registry.context_length_for_model("anthropic/claude-sonnet-4-6"),
            Some(200_000)
        );
        assert!(registry
            .context_length_for_model("nonexistent/model")
            .is_none());
    }

    #[tokio::test]
    async fn models_for_mode_deduplicates() {
        let registry = ModelRegistry::new();
        {
            let mut catalog = registry.catalog.write().await;
            catalog.insert(
                "openai/gpt-4".into(),
                ModelInfo {
                    id: "openai/gpt-4".into(),
                    name: "GPT-4".into(),
                    description: "".into(),
                    context_length: 128_000,
                },
            );
        }
        {
            let mut categories = registry.categories.write().await;
            categories.insert("programming".into(), vec!["openai/gpt-4".into()]);
            categories.insert("technology".into(), vec!["openai/gpt-4".into()]);
        }

        // "plan" merges programming + technology — should deduplicate
        let models = registry.models_for_mode("plan").await;
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "openai/gpt-4");
    }

    #[tokio::test]
    async fn catalog_ids_returns_all_ids() {
        let registry = ModelRegistry::new();
        assert!(registry.catalog_ids().await.is_empty());

        {
            let mut catalog = registry.catalog.write().await;
            catalog.insert(
                "anthropic/claude-sonnet-4-6".into(),
                ModelInfo {
                    id: "anthropic/claude-sonnet-4-6".into(),
                    name: "Claude Sonnet".into(),
                    description: "".into(),
                    context_length: 200_000,
                },
            );
            catalog.insert(
                "openai/gpt-4".into(),
                ModelInfo {
                    id: "openai/gpt-4".into(),
                    name: "GPT-4".into(),
                    description: "".into(),
                    context_length: 128_000,
                },
            );
        }

        let ids = registry.catalog_ids().await;
        assert_eq!(ids.len(), 2);
        assert!(ids.contains("anthropic/claude-sonnet-4-6"));
        assert!(ids.contains("openai/gpt-4"));
    }

    #[tokio::test]
    async fn is_model_known_finds_cached_model() {
        let registry = ModelRegistry::new();
        {
            let mut categories = registry.categories.write().await;
            categories.insert(
                "programming".into(),
                vec!["anthropic/claude-sonnet-4-6".into()],
            );
        }

        assert!(registry.is_model_known("anthropic/claude-sonnet-4-6"));
        assert!(!registry.is_model_known("nonexistent/model"));
    }
}
