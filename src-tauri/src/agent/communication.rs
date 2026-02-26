use serde::{Deserialize, Serialize};
use tracing::{debug, info};
use uuid::Uuid;

use super::types::{AgentRole, TokenMetrics};

/// A compacted summary produced by a sub-agent upon completion.
/// This is what the orchestrator receives and embeds into its own context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSummary {
    pub agent_id: Uuid,
    pub role: AgentRole,
    pub outcome: AgentOutcome,
    pub key_findings: Vec<String>,
    pub files_modified: Vec<String>,
    pub decisions_made: Vec<String>,
    pub learnings: Vec<String>,
    pub token_usage: TokenMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentOutcome {
    Success {
        summary: String,
    },
    PartialSuccess {
        summary: String,
        issues: Vec<String>,
    },
    Failure {
        error: String,
        attempted: String,
    },
}

pub struct SummaryBuilder {
    agent_id: Uuid,
    role: AgentRole,
    key_findings: Vec<String>,
    files_modified: Vec<String>,
    decisions_made: Vec<String>,
    learnings: Vec<String>,
    token_usage: TokenMetrics,
}

impl SummaryBuilder {
    pub fn new(agent_id: Uuid, role: AgentRole) -> Self {
        Self {
            agent_id,
            role,
            key_findings: Vec::new(),
            files_modified: Vec::new(),
            decisions_made: Vec::new(),
            learnings: Vec::new(),
            token_usage: TokenMetrics::default(),
        }
    }

    pub fn add_finding(&mut self, finding: String) -> &mut Self {
        self.key_findings.push(finding);
        self
    }

    pub fn add_file_modified(&mut self, path: String) -> &mut Self {
        self.files_modified.push(path);
        self
    }

    pub fn add_decision(&mut self, decision: String) -> &mut Self {
        self.decisions_made.push(decision);
        self
    }

    pub fn add_learning(&mut self, learning: String) -> &mut Self {
        self.learnings.push(learning);
        self
    }

    pub fn set_token_usage(&mut self, usage: TokenMetrics) -> &mut Self {
        self.token_usage = usage;
        self
    }

    pub fn build_success(self, summary: String) -> AgentSummary {
        info!(agent_id = %self.agent_id, outcome = "success", "building agent summary");
        AgentSummary {
            agent_id: self.agent_id,
            role: self.role,
            outcome: AgentOutcome::Success { summary },
            key_findings: self.key_findings,
            files_modified: self.files_modified,
            decisions_made: self.decisions_made,
            learnings: self.learnings,
            token_usage: self.token_usage,
        }
    }

    pub fn build_partial(self, summary: String, issues: Vec<String>) -> AgentSummary {
        info!(agent_id = %self.agent_id, outcome = "partial", issue_count = issues.len(), "building agent summary");
        AgentSummary {
            agent_id: self.agent_id,
            role: self.role,
            outcome: AgentOutcome::PartialSuccess { summary, issues },
            key_findings: self.key_findings,
            files_modified: self.files_modified,
            decisions_made: self.decisions_made,
            learnings: self.learnings,
            token_usage: self.token_usage,
        }
    }

    pub fn build_failure(self, error: String, attempted: String) -> AgentSummary {
        info!(agent_id = %self.agent_id, outcome = "failure", "building agent summary");
        AgentSummary {
            agent_id: self.agent_id,
            role: self.role,
            outcome: AgentOutcome::Failure { error, attempted },
            key_findings: self.key_findings,
            files_modified: self.files_modified,
            decisions_made: self.decisions_made,
            learnings: self.learnings,
            token_usage: self.token_usage,
        }
    }
}

/// The orchestrator's view of all child agent results.
/// Used to build the orchestrator's context for synthesis.
pub struct OrchestratorContext {
    child_summaries: Vec<AgentSummary>,
}

impl OrchestratorContext {
    pub fn new() -> Self {
        Self {
            child_summaries: Vec::new(),
        }
    }

    /// Add a child agent's summary.
    pub fn add_summary(&mut self, summary: AgentSummary) {
        debug!(agent_id = %summary.agent_id, role = ?summary.role, "adding agent summary");
        self.child_summaries.push(summary);
    }

    /// Format all summaries into a single text block suitable for
    /// embedding into the orchestrator's context/prompt.
    pub fn format_for_context(&self) -> String {
        debug!(
            child_count = self.child_summaries.len(),
            "formatting context"
        );
        let mut output = String::from("## Agent Results\n\n");

        for summary in &self.child_summaries {
            output.push_str(&format!(
                "### {} Agent {}\n",
                role_label(&summary.role),
                short_id(summary.agent_id)
            ));
            output.push_str(&format!(
                "**Outcome**: {}\n",
                outcome_text(&summary.outcome)
            ));
            append_list(&mut output, "Key Findings", &summary.key_findings);
            append_list(&mut output, "Files Modified", &summary.files_modified);
            append_list(&mut output, "Decisions", &summary.decisions_made);
            append_list(&mut output, "Learnings", &summary.learnings);
            output.push('\n');
        }

        output
    }

    /// Get total token usage across all child agents.
    pub fn total_tokens(&self) -> TokenMetrics {
        self.child_summaries
            .iter()
            .fold(TokenMetrics::default(), |acc, summary| {
                acc + summary.token_usage.clone()
            })
    }

    /// Get summaries filtered by role.
    pub fn summaries_by_role(&self, role: &AgentRole) -> Vec<&AgentSummary> {
        self.child_summaries
            .iter()
            .filter(|summary| &summary.role == role)
            .collect()
    }

    /// Get all failed summaries (for orchestrator to decide on retries).
    pub fn failed_summaries(&self) -> Vec<&AgentSummary> {
        let failed: Vec<&AgentSummary> = self
            .child_summaries
            .iter()
            .filter(|summary| matches!(summary.outcome, AgentOutcome::Failure { .. }))
            .collect();
        debug!(count = failed.len(), "failed summaries");
        failed
    }

    /// Get all learnings aggregated across all child agents.
    pub fn all_learnings(&self) -> Vec<String> {
        self.child_summaries
            .iter()
            .flat_map(|summary| summary.learnings.iter().cloned())
            .collect()
    }

    /// Check if all children succeeded.
    pub fn all_succeeded(&self) -> bool {
        let result = self
            .child_summaries
            .iter()
            .all(|summary| matches!(summary.outcome, AgentOutcome::Success { .. }));
        debug!(all_succeeded = result, "all succeeded check");
        result
    }

    /// Count of summaries.
    pub fn count(&self) -> usize {
        self.child_summaries.len()
    }
}

impl Default for OrchestratorContext {
    fn default() -> Self {
        Self::new()
    }
}

impl From<AgentSummary> for super::orchestrator::AgentReport {
    fn from(summary: AgentSummary) -> Self {
        debug!(agent_id = %summary.agent_id, "converting summary to report");
        let (success, summary_text, error) = match &summary.outcome {
            AgentOutcome::Success { summary } => (true, summary.clone(), None),
            AgentOutcome::PartialSuccess { summary, .. } => (true, summary.clone(), None),
            AgentOutcome::Failure { error, .. } => (false, String::new(), Some(error.clone())),
        };

        super::orchestrator::AgentReport {
            agent_id: summary.agent_id,
            summary: summary_text,
            token_usage: summary.token_usage.clone(),
            success,
            error,
        }
    }
}

fn short_id(agent_id: Uuid) -> String {
    agent_id.to_string().chars().take(4).collect()
}

fn role_label(role: &AgentRole) -> &'static str {
    match role {
        AgentRole::Orchestrator => "Orchestrator",
        AgentRole::Research => "Research",
        AgentRole::Implementation => "Implementation",
        AgentRole::Test => "Test",
        AgentRole::Review => "Review",
        AgentRole::Unstuck => "Unstuck",
    }
}

fn outcome_text(outcome: &AgentOutcome) -> String {
    match outcome {
        AgentOutcome::Success { summary } => summary.clone(),
        AgentOutcome::PartialSuccess { summary, issues } => {
            if issues.is_empty() {
                format!("Partial success: {}", summary)
            } else {
                format!("Partial success: {} Issues: {}", summary, issues.join("; "))
            }
        }
        AgentOutcome::Failure { error, attempted } => {
            format!("Failed: {} (attempted: {})", error, attempted)
        }
    }
}

fn append_list(output: &mut String, label: &str, items: &[String]) {
    if items.is_empty() {
        output.push_str(&format!("**{}**: (none)\n", label));
        return;
    }

    output.push_str(&format!("**{}**:\n", label));
    for item in items {
        output.push_str(&format!("- {}\n", item));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tokens(input: u64, output: u64, cost_usd: f64) -> TokenMetrics {
        TokenMetrics {
            input,
            output,
            cost_usd,
        }
    }

    fn summary(agent_id: Uuid, role: AgentRole, outcome: AgentOutcome) -> AgentSummary {
        AgentSummary {
            agent_id,
            role,
            outcome,
            key_findings: Vec::new(),
            files_modified: Vec::new(),
            decisions_made: Vec::new(),
            learnings: Vec::new(),
            token_usage: TokenMetrics::default(),
        }
    }

    #[test]
    fn summary_builder_builds_all_outcome_types() {
        let agent_id = Uuid::new_v4();
        let mut success_builder = SummaryBuilder::new(agent_id, AgentRole::Research);
        success_builder
            .add_finding("Found API docs".to_string())
            .add_file_modified("src/lib.rs".to_string())
            .add_decision("Use API v2".to_string())
            .add_learning("Rate limits are strict".to_string())
            .set_token_usage(sample_tokens(10, 5, 0.01));

        let success = success_builder.build_success("Completed discovery".to_string());
        assert!(matches!(success.outcome, AgentOutcome::Success { .. }));
        assert_eq!(success.key_findings.len(), 1);
        assert_eq!(success.files_modified.len(), 1);
        assert_eq!(success.decisions_made.len(), 1);
        assert_eq!(success.learnings.len(), 1);
        assert_eq!(success.token_usage.input, 10);

        let partial = SummaryBuilder::new(Uuid::new_v4(), AgentRole::Implementation).build_partial(
            "Integrated webhook".to_string(),
            vec!["Missing retries".to_string()],
        );
        match partial.outcome {
            AgentOutcome::PartialSuccess { issues, .. } => assert_eq!(issues.len(), 1),
            _ => panic!("expected partial success"),
        }

        let failure = SummaryBuilder::new(Uuid::new_v4(), AgentRole::Test).build_failure(
            "Tests timed out".to_string(),
            "Ran integration suite".to_string(),
        );
        assert!(matches!(failure.outcome, AgentOutcome::Failure { .. }));
    }

    #[test]
    fn summary_builder_success_preserves_fields() {
        let agent_id = Uuid::new_v4();
        let mut builder = SummaryBuilder::new(agent_id, AgentRole::Implementation);
        builder
            .add_finding("Added orchestration tests".to_string())
            .add_file_modified("src-tauri/src/agent/types.rs".to_string())
            .set_token_usage(sample_tokens(30, 15, 0.004));
        let summary = builder.build_success("Done".to_string());

        assert_eq!(summary.agent_id, agent_id);
        assert_eq!(summary.role, AgentRole::Implementation);
        assert_eq!(summary.key_findings.len(), 1);
        assert_eq!(summary.files_modified.len(), 1);
        assert_eq!(summary.token_usage.input, 30);
        assert!(matches!(summary.outcome, AgentOutcome::Success { .. }));
    }

    #[test]
    fn summary_builder_failure_sets_error_fields() {
        let summary = SummaryBuilder::new(Uuid::new_v4(), AgentRole::Test)
            .build_failure("timeout".to_string(), "integration run".to_string());

        match summary.outcome {
            AgentOutcome::Failure { error, attempted } => {
                assert_eq!(error, "timeout");
                assert_eq!(attempted, "integration run");
            }
            _ => panic!("expected failure outcome"),
        }
    }

    #[test]
    fn summary_builder_partial_sets_issues() {
        let summary = SummaryBuilder::new(Uuid::new_v4(), AgentRole::Review).build_partial(
            "Mostly good".to_string(),
            vec!["One failing check".to_string()],
        );

        match summary.outcome {
            AgentOutcome::PartialSuccess { summary, issues } => {
                assert_eq!(summary, "Mostly good");
                assert_eq!(issues, vec!["One failing check"]);
            }
            _ => panic!("expected partial success"),
        }
    }

    #[test]
    fn format_for_context_is_structured_and_readable() {
        let mut context = OrchestratorContext::new();

        let id_a = Uuid::parse_str("a1b20000-0000-0000-0000-000000000000").unwrap();
        let id_b = Uuid::parse_str("c3d40000-0000-0000-0000-000000000000").unwrap();

        context.add_summary(AgentSummary {
            agent_id: id_a,
            role: AgentRole::Research,
            outcome: AgentOutcome::Success {
                summary: "Identified payment APIs".to_string(),
            },
            key_findings: vec![
                "Stripe supports webhooks".to_string(),
                "PayPal requires OAuth2".to_string(),
            ],
            files_modified: Vec::new(),
            decisions_made: vec!["Recommend Stripe".to_string()],
            learnings: vec!["PayPal sandbox credentials differ".to_string()],
            token_usage: sample_tokens(100, 50, 0.01),
        });
        context.add_summary(AgentSummary {
            agent_id: id_b,
            role: AgentRole::Implementation,
            outcome: AgentOutcome::PartialSuccess {
                summary: "Implemented webhook handler".to_string(),
                issues: vec!["Retry handling not implemented".to_string()],
            },
            key_findings: vec!["Raw request body required for signature checks".to_string()],
            files_modified: vec!["src/webhooks/stripe.rs".to_string()],
            decisions_made: vec!["Use async processing".to_string()],
            learnings: Vec::new(),
            token_usage: sample_tokens(80, 40, 0.008),
        });

        let formatted = context.format_for_context();
        assert!(formatted.contains("## Agent Results"));
        assert!(formatted.contains("### Research Agent a1b2"));
        assert!(formatted.contains("### Implementation Agent c3d4"));
        assert!(formatted.contains("**Outcome**: Identified payment APIs"));
        assert!(formatted.contains("**Files Modified**: (none)"));
        assert!(formatted.contains("- src/webhooks/stripe.rs"));
        assert!(formatted.contains("**Decisions**:"));
        assert!(formatted.contains("**Learnings**:"));
    }

    #[test]
    fn total_tokens_sums_all_children() {
        let mut context = OrchestratorContext::new();
        let mut first = summary(
            Uuid::new_v4(),
            AgentRole::Research,
            AgentOutcome::Success {
                summary: "ok".to_string(),
            },
        );
        first.token_usage = sample_tokens(12, 8, 0.01);

        let mut second = summary(
            Uuid::new_v4(),
            AgentRole::Implementation,
            AgentOutcome::Success {
                summary: "ok".to_string(),
            },
        );
        second.token_usage = sample_tokens(7, 3, 0.005);

        context.add_summary(first);
        context.add_summary(second);

        let total = context.total_tokens();
        assert_eq!(total.input, 19);
        assert_eq!(total.output, 11);
        assert!((total.cost_usd - 0.015).abs() < f64::EPSILON);
    }

    #[test]
    fn failed_summaries_only_returns_failures() {
        let mut context = OrchestratorContext::new();
        context.add_summary(summary(
            Uuid::new_v4(),
            AgentRole::Research,
            AgentOutcome::Success {
                summary: "ok".to_string(),
            },
        ));
        context.add_summary(summary(
            Uuid::new_v4(),
            AgentRole::Test,
            AgentOutcome::Failure {
                error: "network".to_string(),
                attempted: "integration tests".to_string(),
            },
        ));

        let failed = context.failed_summaries();
        assert_eq!(failed.len(), 1);
        assert!(matches!(failed[0].outcome, AgentOutcome::Failure { .. }));
    }

    #[test]
    fn all_learnings_aggregates_from_all_children() {
        let mut context = OrchestratorContext::new();
        let mut first = summary(
            Uuid::new_v4(),
            AgentRole::Research,
            AgentOutcome::Success {
                summary: "ok".to_string(),
            },
        );
        first.learnings = vec!["API pagination limit is 100".to_string()];
        let mut second = summary(
            Uuid::new_v4(),
            AgentRole::Review,
            AgentOutcome::Success {
                summary: "ok".to_string(),
            },
        );
        second.learnings = vec!["Rustfmt changed import order".to_string()];

        context.add_summary(first);
        context.add_summary(second);

        let learnings = context.all_learnings();
        assert_eq!(learnings.len(), 2);
        assert!(learnings.contains(&"API pagination limit is 100".to_string()));
        assert!(learnings.contains(&"Rustfmt changed import order".to_string()));
    }

    #[test]
    fn all_succeeded_requires_pure_success_outcomes() {
        let mut context = OrchestratorContext::new();
        assert!(context.all_succeeded());

        context.add_summary(summary(
            Uuid::new_v4(),
            AgentRole::Research,
            AgentOutcome::Success {
                summary: "ok".to_string(),
            },
        ));
        assert!(context.all_succeeded());

        context.add_summary(summary(
            Uuid::new_v4(),
            AgentRole::Implementation,
            AgentOutcome::PartialSuccess {
                summary: "mostly done".to_string(),
                issues: vec!["missing tests".to_string()],
            },
        ));
        assert!(!context.all_succeeded());
    }

    #[test]
    fn summary_converts_to_agent_report() {
        let success_summary = AgentSummary {
            agent_id: Uuid::new_v4(),
            role: AgentRole::Test,
            outcome: AgentOutcome::Success {
                summary: "All checks passed".to_string(),
            },
            key_findings: Vec::new(),
            files_modified: Vec::new(),
            decisions_made: Vec::new(),
            learnings: Vec::new(),
            token_usage: sample_tokens(20, 10, 0.02),
        };

        let success_report: super::super::orchestrator::AgentReport = success_summary.into();
        assert!(success_report.success);
        assert_eq!(success_report.summary, "All checks passed");
        assert!(success_report.error.is_none());

        let failure_summary = AgentSummary {
            agent_id: Uuid::new_v4(),
            role: AgentRole::Test,
            outcome: AgentOutcome::Failure {
                error: "panic".to_string(),
                attempted: "cargo test".to_string(),
            },
            key_findings: Vec::new(),
            files_modified: Vec::new(),
            decisions_made: Vec::new(),
            learnings: Vec::new(),
            token_usage: sample_tokens(20, 10, 0.02),
        };

        let failure_report: super::super::orchestrator::AgentReport = failure_summary.into();
        assert!(!failure_report.success);
        assert_eq!(failure_report.summary, "");
        assert_eq!(failure_report.error.as_deref(), Some("panic"));
    }

    #[test]
    fn summary_to_agent_report_success() {
        let summary = AgentSummary {
            agent_id: Uuid::new_v4(),
            role: AgentRole::Research,
            outcome: AgentOutcome::Success {
                summary: "Found root cause".to_string(),
            },
            key_findings: Vec::new(),
            files_modified: Vec::new(),
            decisions_made: Vec::new(),
            learnings: Vec::new(),
            token_usage: sample_tokens(11, 9, 0.002),
        };

        let report: super::super::orchestrator::AgentReport = summary.into();
        assert!(report.success);
        assert_eq!(report.summary, "Found root cause");
        assert!(report.error.is_none());
    }

    #[test]
    fn summary_to_agent_report_failure() {
        let summary = AgentSummary {
            agent_id: Uuid::new_v4(),
            role: AgentRole::Research,
            outcome: AgentOutcome::Failure {
                error: "request failed".to_string(),
                attempted: "web search".to_string(),
            },
            key_findings: Vec::new(),
            files_modified: Vec::new(),
            decisions_made: Vec::new(),
            learnings: Vec::new(),
            token_usage: sample_tokens(11, 9, 0.002),
        };

        let report: super::super::orchestrator::AgentReport = summary.into();
        assert!(!report.success);
        assert_eq!(report.summary, "");
        assert_eq!(report.error.as_deref(), Some("request failed"));
    }

    #[test]
    fn summaries_can_be_filtered_by_role_and_counted() {
        let mut context = OrchestratorContext::new();
        context.add_summary(summary(
            Uuid::new_v4(),
            AgentRole::Research,
            AgentOutcome::Success {
                summary: "ok".to_string(),
            },
        ));
        context.add_summary(summary(
            Uuid::new_v4(),
            AgentRole::Research,
            AgentOutcome::Success {
                summary: "ok".to_string(),
            },
        ));
        context.add_summary(summary(
            Uuid::new_v4(),
            AgentRole::Implementation,
            AgentOutcome::Success {
                summary: "ok".to_string(),
            },
        ));

        let research = context.summaries_by_role(&AgentRole::Research);
        assert_eq!(research.len(), 2);
        assert_eq!(context.count(), 3);
    }

    #[test]
    fn format_contains_short_id() {
        let fixed_id = Uuid::parse_str("abcd1234-1111-2222-3333-444455556666").unwrap();
        let mut context = OrchestratorContext::new();
        context.add_summary(summary(
            fixed_id,
            AgentRole::Research,
            AgentOutcome::Success {
                summary: "ok".to_string(),
            },
        ));

        let formatted = context.format_for_context();
        assert!(formatted.contains("### Research Agent abcd"));
    }
}
