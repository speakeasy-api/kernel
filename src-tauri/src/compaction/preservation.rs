use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use super::Message;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PreservedPattern {
    /// File paths (e.g., /foo/bar.rs, ./src/main.rs)
    FilePath,
    /// Function/method signatures (e.g., fn foo(), pub fn bar(x: i32) -> bool)
    FunctionSignature,
    /// Error messages (e.g., "error[E0308]: mismatched types")
    ErrorMessage,
    /// Decision records (text marked with "Decision:" or "Decided:" prefix)
    DecisionRecord,
    /// Current task state (text marked with "Task:" or "Current task:" prefix)
    TaskState,
    /// Custom pattern with a description (for documentation/LLM prompting only)
    Custom(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PreservationRules {
    /// Patterns that indicate preserved content (never removed/summarized)
    pub preserved_patterns: Vec<PreservedPattern>,
}

impl PreservationRules {
    /// Returns the default preservation rules as specified in the compaction spec.
    pub fn default_rules() -> Self {
        Self {
            preserved_patterns: vec![
                PreservedPattern::FilePath,
                PreservedPattern::FunctionSignature,
                PreservedPattern::ErrorMessage,
                PreservedPattern::DecisionRecord,
                PreservedPattern::TaskState,
            ],
        }
    }

    /// Extract all preserved facts from a slice of messages.
    /// Returns a deduplicated list of strings that must be preserved during compaction.
    #[instrument(skip_all, fields(message_count = messages.len(), pattern_count = self.preserved_patterns.len()))]
    pub fn extract_preserved_facts(&self, messages: &[Message]) -> Vec<String> {
        let mut facts = Vec::new();
        for msg in messages {
            for pattern in &self.preserved_patterns {
                facts.extend(extract_for_pattern(pattern, &msg.content));
            }
        }
        facts.sort();
        facts.dedup();
        debug!(facts_count = facts.len(), "extracted preserved facts");
        facts
    }

}

fn extract_for_pattern(pattern: &PreservedPattern, content: &str) -> Vec<String> {
    match pattern {
        PreservedPattern::FilePath => extract_file_paths(content),
        PreservedPattern::FunctionSignature => extract_function_signatures(content),
        PreservedPattern::ErrorMessage => extract_error_messages(content),
        PreservedPattern::DecisionRecord => extract_decision_records(content),
        PreservedPattern::TaskState => extract_task_state(content),
        PreservedPattern::Custom(_) => Vec::new(),
    }
}

fn extract_file_paths(content: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for token in content.split_whitespace() {
        // Strip common surrounding punctuation
        let token = token.trim_matches(|c: char| {
            c == ',' || c == ';' || c == '"' || c == '\'' || c == '(' || c == ')' || c == '`'
        });
        if token.is_empty() {
            continue;
        }
        // Skip URLs
        if token.contains("://") {
            continue;
        }
        // Must contain a `/` to look like a path
        if !token.contains('/') {
            continue;
        }
        // Must start with `/`, `./`, or `../` to be a recognizable path
        if token.starts_with('/') || token.starts_with("./") || token.starts_with("../") {
            // Check it has at least one segment after the leading slash/dot
            let has_extension = token
                .rsplit('/')
                .next()
                .map_or(false, |last| last.contains('.'));
            let has_multiple_segments = token.matches('/').count() >= 1
                && token
                    .split('/')
                    .any(|seg| !seg.is_empty() && seg != "." && seg != "..");
            if has_extension || has_multiple_segments {
                paths.push(token.to_string());
            }
        }
    }
    paths
}

fn extract_function_signatures(content: &str) -> Vec<String> {
    let mut sigs = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // Rust: fn keyword
        if contains_fn_keyword(trimmed) {
            sigs.push(trimmed.to_string());
            continue;
        }
        // Python: def keyword
        if trimmed.starts_with("def ")
            || trimmed.starts_with("async def ")
            || (trimmed.contains("def ")
                && trimmed
                    .trim_start_matches(|c: char| c.is_whitespace())
                    .starts_with("def "))
        {
            if trimmed.contains('(') {
                sigs.push(trimmed.to_string());
                continue;
            }
        }
        // TypeScript/JavaScript: function keyword
        if trimmed.starts_with("function ")
            || trimmed.starts_with("async function ")
            || trimmed.starts_with("export function ")
            || trimmed.starts_with("export async function ")
            || trimmed.starts_with("export default function ")
        {
            sigs.push(trimmed.to_string());
            continue;
        }
    }
    sigs
}

/// Check if a line contains a Rust fn definition.
fn contains_fn_keyword(line: &str) -> bool {
    // Match: fn, pub fn, pub(crate) fn, async fn, pub async fn, etc.
    let prefixes = [
        "fn ",
        "pub fn ",
        "pub(crate) fn ",
        "pub(super) fn ",
        "async fn ",
        "pub async fn ",
        "pub(crate) async fn ",
        "pub(super) async fn ",
        "unsafe fn ",
        "pub unsafe fn ",
        "pub(crate) unsafe fn ",
        "const fn ",
        "pub const fn ",
        "pub(crate) const fn ",
    ];
    for prefix in &prefixes {
        if line.starts_with(prefix) {
            return true;
        }
    }
    false
}

fn extract_error_messages(content: &str) -> Vec<String> {
    let mut errors = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_lowercase();
        if lower.starts_with("error")
            || lower.starts_with("panic!")
            || lower.starts_with("thread '")
            || starts_with_any(&lower, &["failed", "exception", "traceback"])
        {
            errors.push(trimmed.to_string());
        }
    }
    errors
}

fn extract_decision_records(content: &str) -> Vec<String> {
    let prefixes = [
        "decision:",
        "decided:",
        "we decided",
        "i decided",
        "rationale:",
    ];
    extract_paragraphs_by_prefix(content, &prefixes)
}

fn extract_task_state(content: &str) -> Vec<String> {
    let prefixes = ["task:", "current task:", "todo:", "working on:", "next:"];
    let mut results = Vec::new();
    for line in content.lines() {
        let lower = line.trim().to_lowercase();
        if prefixes.iter().any(|p| lower.starts_with(p)) {
            results.push(line.trim().to_string());
        }
    }
    results
}

/// Extract full paragraphs (up to next blank line) for lines matching any prefix.
fn extract_paragraphs_by_prefix(content: &str, prefixes: &[&str]) -> Vec<String> {
    let mut results = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let lower = lines[i].trim().to_lowercase();
        if prefixes.iter().any(|p| lower.starts_with(p)) {
            let mut paragraph = String::from(lines[i].trim());
            i += 1;
            // Collect continuation lines until blank line or end
            while i < lines.len() && !lines[i].trim().is_empty() {
                paragraph.push('\n');
                paragraph.push_str(lines[i].trim());
                i += 1;
            }
            results.push(paragraph);
        } else {
            i += 1;
        }
    }
    results
}

fn starts_with_any(s: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| s.starts_with(p))
}
