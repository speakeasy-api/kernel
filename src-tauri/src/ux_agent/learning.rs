use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument};

use super::store::RecommendationStore;

/// Minimum number of identical corrected_value occurrences to extract a convention.
const PATTERN_THRESHOLD: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Correction {
    pub id: u64,
    pub session_id: Option<String>,
    pub correction_type: CorrectionType,
    pub original_value: Option<String>,
    pub corrected_value: Option<String>,
    pub context: Option<String>,
    pub created_at: String,
    pub incorporated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CorrectionType {
    ModeOverride,
    PlanRejection,
    DiffRejection,
    HunkRejection,
    AgentSteering,
}

impl CorrectionType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::ModeOverride => "mode_override",
            Self::PlanRejection => "plan_rejection",
            Self::DiffRejection => "diff_rejection",
            Self::HunkRejection => "hunk_rejection",
            Self::AgentSteering => "agent_steering",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "mode_override" => Some(Self::ModeOverride),
            "plan_rejection" => Some(Self::PlanRejection),
            "diff_rejection" => Some(Self::DiffRejection),
            "hunk_rejection" => Some(Self::HunkRejection),
            "agent_steering" => Some(Self::AgentSteering),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Convention {
    pub id: u64,
    pub convention: String,
    pub source_corrections: Vec<u64>,
    pub target_mode: Option<String>,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ExtractedConvention {
    pub convention: String,
    pub correction_ids: Vec<u64>,
    pub target_mode: Option<String>,
    pub confidence: f32,
}

/// Analyze unincorporated corrections and extract conventions.
///
/// Groups corrections by type, looks for repeated patterns in corrected values,
/// and produces convention statements with confidence scores.
#[instrument(skip(corrections), fields(correction_count = corrections.len()))]
pub fn extract_conventions(corrections: &[Correction]) -> Vec<ExtractedConvention> {
    debug!(correction_count = corrections.len(), "analyzing corrections for convention extraction");
    let mut extracted = Vec::new();

    // Group corrections by type.
    let mut by_type: HashMap<&str, Vec<&Correction>> = HashMap::new();
    for c in corrections {
        by_type.entry(c.correction_type.as_str()).or_default().push(c);
    }

    // Mode overrides: look for repeated from->to patterns.
    if let Some(overrides) = by_type.get("mode_override") {
        extracted.extend(extract_mode_override_conventions(overrides));
    }

    // Rejections: cluster by corrected_value substring similarity.
    for rejection_type in &["plan_rejection", "diff_rejection", "hunk_rejection"] {
        if let Some(rejections) = by_type.get(rejection_type) {
            extracted.extend(extract_rejection_conventions(rejections, rejection_type));
        }
    }

    // Agent steering: extract common instructions.
    if let Some(steerings) = by_type.get("agent_steering") {
        extracted.extend(extract_steering_conventions(steerings));
    }

    if !extracted.is_empty() {
        info!(
            convention_count = extracted.len(),
            "patterns detected from corrections"
        );
        for conv in &extracted {
            info!(
                convention = %conv.convention,
                confidence = conv.confidence,
                correction_ids = ?conv.correction_ids,
                "extracted convention"
            );
        }
    } else {
        debug!("no conventions extracted from corrections");
    }

    extracted
}

#[instrument(skip(overrides), fields(override_count = overrides.len()))]
fn extract_mode_override_conventions(overrides: &[&Correction]) -> Vec<ExtractedConvention> {
    debug!(override_count = overrides.len(), "extracting mode override conventions");
    // Group by (original_value, corrected_value) pair.
    let mut pattern_groups: HashMap<(String, String), Vec<u64>> = HashMap::new();
    for c in overrides {
        let from = c.original_value.clone().unwrap_or_default();
        let to = c.corrected_value.clone().unwrap_or_default();
        pattern_groups.entry((from, to)).or_default().push(c.id);
    }

    let total = overrides.len();
    pattern_groups
        .into_iter()
        .filter(|(_, ids)| ids.len() >= PATTERN_THRESHOLD)
        .map(|((from, to), ids)| {
            let confidence = ids.len() as f32 / total as f32;
            ExtractedConvention {
                convention: format!("prefer mode '{to}' over '{from}'"),
                correction_ids: ids,
                target_mode: Some(to),
                confidence,
            }
        })
        .collect()
}

#[instrument(skip(rejections), fields(rejection_count = rejections.len()))]
fn extract_rejection_conventions(
    rejections: &[&Correction],
    rejection_type: &str,
) -> Vec<ExtractedConvention> {
    debug!(rejection_count = rejections.len(), rejection_type, "extracting rejection conventions");
    // Group by corrected_value (user feedback) using exact match for v1.
    let mut feedback_groups: HashMap<String, Vec<u64>> = HashMap::new();
    for c in rejections {
        if let Some(ref feedback) = c.corrected_value {
            if !feedback.is_empty() {
                feedback_groups
                    .entry(feedback.clone())
                    .or_default()
                    .push(c.id);
            }
        }
    }

    let label = match rejection_type {
        "plan_rejection" => "plans",
        "diff_rejection" => "diffs",
        "hunk_rejection" => "hunks",
        _ => "outputs",
    };

    let total = rejections.len().max(1);
    feedback_groups
        .into_iter()
        .filter(|(_, ids)| ids.len() >= PATTERN_THRESHOLD)
        .map(|(feedback, ids)| {
            let confidence = ids.len() as f32 / total as f32;
            // Derive target mode from context if available.
            let target_mode = None;
            ExtractedConvention {
                convention: format!("when generating {label}: {feedback}"),
                correction_ids: ids,
                target_mode,
                confidence,
            }
        })
        .collect()
}

#[instrument(skip(steerings), fields(steering_count = steerings.len()))]
fn extract_steering_conventions(steerings: &[&Correction]) -> Vec<ExtractedConvention> {
    debug!(steering_count = steerings.len(), "extracting steering conventions");
    // Group by corrected_value (the steering instruction).
    let mut instruction_groups: HashMap<String, Vec<u64>> = HashMap::new();
    for c in steerings {
        if let Some(ref instruction) = c.corrected_value {
            if !instruction.is_empty() {
                instruction_groups
                    .entry(instruction.clone())
                    .or_default()
                    .push(c.id);
            }
        }
    }

    let total = steerings.len().max(1);
    instruction_groups
        .into_iter()
        .filter(|(_, ids)| ids.len() >= PATTERN_THRESHOLD)
        .map(|(instruction, ids)| {
            let confidence = ids.len() as f32 / total as f32;
            ExtractedConvention {
                convention: instruction,
                correction_ids: ids,
                target_mode: None,
                confidence,
            }
        })
        .collect()
}

/// Build a learning summary for inclusion in the UX agent prompt.
///
/// Returns a structured string describing unincorporated corrections and
/// proposed conventions.
#[instrument(skip(store))]
pub async fn build_learning_context(
    store: &RecommendationStore,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    debug!("building learning context from store");
    let corrections = store.get_unincorporated_corrections().await?;
    let conventions = extract_conventions(&corrections);
    let proposed = store.get_proposed_conventions().await?;

    debug!(
        corrections = corrections.len(),
        extracted_conventions = conventions.len(),
        proposed_conventions = proposed.len(),
        "learning context assembled"
    );

    Ok(format_learning_context(&corrections, &conventions, &proposed))
}

#[instrument(skip(corrections, extracted, proposed), fields(
    corrections = corrections.len(),
    extracted = extracted.len(),
    proposed = proposed.len()
))]
fn format_learning_context(
    corrections: &[Correction],
    extracted: &[ExtractedConvention],
    proposed: &[Convention],
) -> String {
    debug!(
        corrections = corrections.len(),
        extracted = extracted.len(),
        proposed = proposed.len(),
        "formatting learning context"
    );
    if corrections.is_empty() && extracted.is_empty() && proposed.is_empty() {
        return String::new();
    }

    let mut out = String::new();

    if !corrections.is_empty() {
        out.push_str("Corrections since last incorporation:\n");
        let mut counts: HashMap<&str, usize> = HashMap::new();
        let mut mode_overrides: HashMap<(String, String), usize> = HashMap::new();
        let mut rejection_feedback: HashMap<String, usize> = HashMap::new();

        for c in corrections {
            *counts.entry(c.correction_type.as_str()).or_default() += 1;
            if c.correction_type == CorrectionType::ModeOverride {
                let from = c.original_value.clone().unwrap_or_default();
                let to = c.corrected_value.clone().unwrap_or_default();
                *mode_overrides.entry((from, to)).or_default() += 1;
            }
            if matches!(
                c.correction_type,
                CorrectionType::PlanRejection
                    | CorrectionType::DiffRejection
                    | CorrectionType::HunkRejection
            ) {
                if let Some(ref fb) = c.corrected_value {
                    if !fb.is_empty() {
                        *rejection_feedback.entry(fb.clone()).or_default() += 1;
                    }
                }
            }
        }

        for (ctype, count) in &counts {
            out.push_str(&format!("  - {count} {ctype}"));
            if *ctype == "mode_override" {
                let details: Vec<String> = mode_overrides
                    .iter()
                    .map(|((from, to), n)| format!("{n} from {from} to {to}"))
                    .collect();
                if !details.is_empty() {
                    out.push_str(&format!(" ({})", details.join(", ")));
                }
            }
            out.push('\n');
        }

        if !rejection_feedback.is_empty() {
            let feedback_list: Vec<String> = rejection_feedback
                .iter()
                .map(|(fb, n)| format!("'{fb}' x{n}"))
                .collect();
            out.push_str(&format!(
                "  Rejection feedback: {}\n",
                feedback_list.join(", ")
            ));
        }
    }

    if !extracted.is_empty() {
        out.push_str("Extracted patterns:\n");
        for conv in extracted {
            out.push_str(&format!(
                "  - {} (confidence: {:.1})\n",
                conv.convention, conv.confidence
            ));
        }
    }

    if !proposed.is_empty() {
        out.push_str("Previously proposed conventions (pending review):\n");
        for conv in proposed {
            let scope = conv
                .target_mode
                .as_deref()
                .map(|m| format!(" [mode: {m}]"))
                .unwrap_or_default();
            out.push_str(&format!("  - {}{}\n", conv.convention, scope));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_pool;
    use crate::ux_agent::store::RecommendationStore;
    use sqlx::SqlitePool;

    async fn setup() -> SqlitePool {
        test_pool().await
    }

    fn make_correction(
        correction_type: CorrectionType,
        original: Option<&str>,
        corrected: Option<&str>,
    ) -> Correction {
        Correction {
            id: 0,
            session_id: Some("test-session".to_string()),
            correction_type,
            original_value: original.map(|s| s.to_string()),
            corrected_value: corrected.map(|s| s.to_string()),
            context: None,
            created_at: String::new(),
            incorporated: false,
        }
    }

    #[test]
    fn correction_type_roundtrip() {
        for ct in [
            CorrectionType::ModeOverride,
            CorrectionType::PlanRejection,
            CorrectionType::DiffRejection,
            CorrectionType::HunkRejection,
            CorrectionType::AgentSteering,
        ] {
            let s = ct.as_str();
            let parsed = CorrectionType::from_str(s).unwrap();
            assert_eq!(parsed, ct);
        }
    }

    #[test]
    fn correction_type_unknown_returns_none() {
        assert!(CorrectionType::from_str("nonexistent").is_none());
    }

    #[tokio::test]
    async fn insert_and_retrieve_correction() {
        let pool = setup().await;
        let store = RecommendationStore::new(pool.clone());
        let c = make_correction(CorrectionType::ModeOverride, Some("General"), Some("Implement"));

        let id = store.insert_correction(&c).await.unwrap();
        assert!(id > 0);

        let all = store.get_unincorporated_corrections().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, id);
        assert_eq!(all[0].correction_type, CorrectionType::ModeOverride);
        assert_eq!(all[0].original_value.as_deref(), Some("General"));
        assert_eq!(all[0].corrected_value.as_deref(), Some("Implement"));
        assert!(!all[0].incorporated);
    }

    #[tokio::test]
    async fn get_corrections_by_type_filters() {
        let pool = setup().await;
        let store = RecommendationStore::new(pool.clone());

        store
            .insert_correction(&make_correction(
                CorrectionType::ModeOverride,
                Some("A"),
                Some("B"),
            ))
            .await
            .unwrap();
        store
            .insert_correction(&make_correction(
                CorrectionType::PlanRejection,
                None,
                Some("too verbose"),
            ))
            .await
            .unwrap();

        let overrides = store.get_corrections_by_type("mode_override").await.unwrap();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].correction_type, CorrectionType::ModeOverride);

        let rejections = store.get_corrections_by_type("plan_rejection").await.unwrap();
        assert_eq!(rejections.len(), 1);
    }

    #[tokio::test]
    async fn mark_corrections_incorporated() {
        let pool = setup().await;
        let store = RecommendationStore::new(pool.clone());

        let id1 = store
            .insert_correction(&make_correction(
                CorrectionType::ModeOverride,
                Some("A"),
                Some("B"),
            ))
            .await
            .unwrap();
        let id2 = store
            .insert_correction(&make_correction(
                CorrectionType::DiffRejection,
                None,
                Some("bad"),
            ))
            .await
            .unwrap();

        store.mark_corrections_incorporated(&[id1]).await.unwrap();

        let unincorporated = store.get_unincorporated_corrections().await.unwrap();
        assert_eq!(unincorporated.len(), 1);
        assert_eq!(unincorporated[0].id, id2);
    }

    #[tokio::test]
    async fn mark_corrections_incorporated_empty_ids() {
        let pool = setup().await;
        let store = RecommendationStore::new(pool.clone());
        // Should not error on empty list.
        store.mark_corrections_incorporated(&[]).await.unwrap();
    }

    #[tokio::test]
    async fn convention_lifecycle() {
        let pool = setup().await;
        let store = RecommendationStore::new(pool.clone());

        let id = store
            .insert_convention("always use migrations", &[1, 2, 3], Some("implement"))
            .await
            .unwrap();
        assert!(id > 0);

        let proposed = store.get_proposed_conventions().await.unwrap();
        assert_eq!(proposed.len(), 1);
        assert_eq!(proposed[0].convention, "always use migrations");
        assert_eq!(proposed[0].source_corrections, vec![1, 2, 3]);
        assert_eq!(proposed[0].target_mode.as_deref(), Some("implement"));
        assert_eq!(proposed[0].status, "proposed");

        store.update_convention_status(id, "applied").await.unwrap();
        let proposed_after = store.get_proposed_conventions().await.unwrap();
        assert!(proposed_after.is_empty());

        // Insert another and dismiss it.
        let id2 = store
            .insert_convention("never alter tables directly", &[4], None)
            .await
            .unwrap();
        store.update_convention_status(id2, "dismissed").await.unwrap();
        let proposed_final = store.get_proposed_conventions().await.unwrap();
        assert!(proposed_final.is_empty());
    }

    #[test]
    fn extract_conventions_mode_overrides() {
        let corrections: Vec<Correction> = (0..4)
            .map(|i| Correction {
                id: i,
                session_id: None,
                correction_type: CorrectionType::ModeOverride,
                original_value: Some("General".to_string()),
                corrected_value: Some("Implement".to_string()),
                context: None,
                created_at: String::new(),
                incorporated: false,
            })
            .collect();

        let conventions = extract_conventions(&corrections);
        assert_eq!(conventions.len(), 1);
        assert!(conventions[0].convention.contains("Implement"));
        assert!(conventions[0].convention.contains("General"));
        assert_eq!(conventions[0].correction_ids.len(), 4);
        assert_eq!(conventions[0].target_mode.as_deref(), Some("Implement"));
        assert!(conventions[0].confidence > 0.0);
    }

    #[test]
    fn extract_conventions_below_threshold_ignored() {
        let corrections: Vec<Correction> = (0..2)
            .map(|i| Correction {
                id: i,
                session_id: None,
                correction_type: CorrectionType::ModeOverride,
                original_value: Some("A".to_string()),
                corrected_value: Some("B".to_string()),
                context: None,
                created_at: String::new(),
                incorporated: false,
            })
            .collect();

        let conventions = extract_conventions(&corrections);
        assert!(conventions.is_empty());
    }

    #[test]
    fn extract_conventions_rejections_with_feedback() {
        let corrections: Vec<Correction> = (0..3)
            .map(|i| Correction {
                id: i,
                session_id: None,
                correction_type: CorrectionType::PlanRejection,
                original_value: None,
                corrected_value: Some("too verbose".to_string()),
                context: None,
                created_at: String::new(),
                incorporated: false,
            })
            .collect();

        let conventions = extract_conventions(&corrections);
        assert_eq!(conventions.len(), 1);
        assert!(conventions[0].convention.contains("too verbose"));
        assert!(conventions[0].convention.contains("plans"));
    }

    #[test]
    fn extract_conventions_steering() {
        let corrections: Vec<Correction> = (0..3)
            .map(|i| Correction {
                id: i,
                session_id: None,
                correction_type: CorrectionType::AgentSteering,
                original_value: None,
                corrected_value: Some("always include tests".to_string()),
                context: None,
                created_at: String::new(),
                incorporated: false,
            })
            .collect();

        let conventions = extract_conventions(&corrections);
        assert_eq!(conventions.len(), 1);
        assert_eq!(conventions[0].convention, "always include tests");
    }

    #[test]
    fn extract_conventions_empty_input() {
        let conventions = extract_conventions(&[]);
        assert!(conventions.is_empty());
    }

    #[tokio::test]
    async fn build_learning_context_produces_output() {
        let pool = setup().await;
        let store = RecommendationStore::new(pool.clone());

        for _ in 0..3 {
            store
                .insert_correction(&make_correction(
                    CorrectionType::ModeOverride,
                    Some("General"),
                    Some("Implement"),
                ))
                .await
                .unwrap();
        }
        store
            .insert_convention("always use migrations", &[1, 2], Some("implement"))
            .await
            .unwrap();

        let context = build_learning_context(&store).await.unwrap();
        assert!(context.contains("Corrections since last incorporation"));
        assert!(context.contains("mode_override"));
        assert!(context.contains("Previously proposed conventions"));
        assert!(context.contains("always use migrations"));
    }

    #[tokio::test]
    async fn build_learning_context_empty_when_nothing() {
        let pool = setup().await;
        let store = RecommendationStore::new(pool.clone());

        let context = build_learning_context(&store).await.unwrap();
        assert!(context.is_empty());
    }

    #[test]
    fn format_learning_context_includes_extracted() {
        let corrections: Vec<Correction> = (0..4)
            .map(|i| Correction {
                id: i,
                session_id: None,
                correction_type: CorrectionType::ModeOverride,
                original_value: Some("General".to_string()),
                corrected_value: Some("Implement".to_string()),
                context: None,
                created_at: String::new(),
                incorporated: false,
            })
            .collect();

        let extracted = extract_conventions(&corrections);
        let output = format_learning_context(&corrections, &extracted, &[]);
        assert!(output.contains("Extracted patterns"));
        assert!(output.contains("confidence"));
    }
}
