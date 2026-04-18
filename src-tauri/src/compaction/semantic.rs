//! Semantic compaction primitives reused by the agentkit_bridge backend.
//!
//! This module exposes only the prompt-building and response-parsing helpers
//! plus the `CompactionError` type. The orchestration (LLM call, DB writes,
//! turn loop integration) lives in `agentkit_bridge::compaction::backend`.

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("Failed to parse compactor response: {0}")]
    ParseFailed(String),
}

#[derive(Debug, Deserialize)]
pub struct CompactorResponse {
    pub messages: Vec<CompactorMessage>,
    pub learnings: Vec<String>,
    pub preserved_facts: Vec<String>,
    #[serde(default)]
    pub pinned_snippets: Option<Vec<RawPinnedSnippet>>,
}

#[derive(Debug, Deserialize)]
pub struct RawPinnedSnippet {
    pub position: usize,
    pub snippet: String,
}

#[derive(Debug, Deserialize)]
pub struct CompactorMessage {
    pub role: String,
    pub content: String,
}

/// Reference to a pinned message that the backend has access to but is not
/// summarising. Carries the index that the LLM should refer to in its
/// `pinned_snippets` output and the message body for context.
pub struct PinnedReference {
    pub position: usize,
    pub role: String,
    pub content: String,
}

use super::budget::Message;

/// Build the (system_prompt, user_prompt) pair for a compaction LLM call.
///
/// `messages` is the slice being compacted (excludes pinned messages).
/// `target_tokens` is the soft target the model should aim for.
/// `preserved_items` is the deduped list extracted via `PreservationRules`.
/// `newly_pinned` are messages that need a context snippet generated; the
/// model emits a `pinned_snippets` array using their `position` field.
pub fn build_compaction_prompt(
    messages: &[Message],
    target_tokens: usize,
    preserved_items: &[String],
    newly_pinned: &[PinnedReference],
) -> (String, String) {
    let pinned_snippet_rule = if !newly_pinned.is_empty() {
        "\n- \"pinned_snippets\": (optional) an array of objects with \"position\" (integer) and \"snippet\" (string) for each newly pinned message that needs surrounding context to be self-contained. Use empty string if the message is already self-contained.\n"
    } else {
        ""
    };

    let system_prompt = format!(
        "You are a context compactor. Your job is to compress a conversation while preserving all critical information.\n\
        \n\
        Rules:\n\
        - NEVER remove or summarize: file paths, function signatures, error messages, decisions and rationale, current task state\n\
        - Extract learnings from failed tool calls as: \"LEARNING: tried X, failed because Y — avoid Z\"\n\
        - Summarize completed reasoning chains into concise conclusions\n\
        - Remove superseded logic ONLY when you are certain it is no longer relevant\n\
        - Preserve the chronological flow of the conversation\n\
        - Target approximately {target_tokens} tokens in your output\n\
        \n\
        Output format:\n\
        Emit a JSON object with these fields:\n\
        - \"messages\": an array of objects with \"role\" and \"content\" fields\n\
        - \"learnings\": an array of strings extracted from failed attempts\n\
        - \"preserved_facts\": an array of strings (file paths, signatures, etc.)\
        {pinned_snippet_rule}\n\
        Output ONLY the JSON object, no markdown fences or other text."
    );

    let mut user_prompt = String::new();

    user_prompt.push_str("## Messages to compact\n\n");
    for msg in messages {
        user_prompt.push_str(&format!("[{}]: {}\n\n", msg.role, msg.content));
    }

    if !preserved_items.is_empty() {
        user_prompt.push_str("## Items that MUST be preserved\n\n");
        for item in preserved_items {
            user_prompt.push_str(&format!("- {item}\n"));
        }
        user_prompt.push('\n');
    }

    if !newly_pinned.is_empty() {
        user_prompt.push_str("## Pinned messages (DO NOT include in compacted output — preserved separately)\n\n");
        user_prompt.push_str(
            "These messages are pinned by the user and will be kept verbatim. However, some may\n\
             reference surrounding context (e.g., \"stop doing that\", \"remember this ^\") that will\n\
             be lost after compaction. For each pinned message below, determine if it needs a\n\
             context snippet to be self-contained. Output a \"pinned_snippets\" key in your response.\n\n",
        );
        for pinned in newly_pinned {
            user_prompt.push_str(&format!(
                "[{} @ position {}]: \"{}\"\n",
                pinned.role, pinned.position, pinned.content
            ));
        }
        user_prompt.push('\n');
    }

    user_prompt.push_str(&format!(
        "## Target\n\nApproximately {target_tokens} tokens.\n"
    ));

    (system_prompt, user_prompt)
}

/// Parse a raw LLM response (possibly wrapped in a fenced code block) into
/// the structured compaction payload.
pub fn parse_compactor_response(response: &str) -> Result<CompactorResponse, CompactionError> {
    if let Ok(parsed) = serde_json::from_str::<CompactorResponse>(response) {
        return Ok(parsed);
    }

    if let Some(json_str) = extract_json_from_codeblock(response) {
        if let Ok(parsed) = serde_json::from_str::<CompactorResponse>(json_str) {
            return Ok(parsed);
        }
    }

    Err(CompactionError::ParseFailed(format!(
        "could not parse compactor response as JSON: {}",
        &response[..response.len().min(200)]
    )))
}

fn extract_json_from_codeblock(text: &str) -> Option<&str> {
    let start = if let Some(pos) = text.find("```json") {
        pos + "```json".len()
    } else if let Some(pos) = text.find("```") {
        pos + "```".len()
    } else {
        return None;
    };

    let rest = &text[start..];
    let end = rest.find("```")?;
    Some(rest[..end].trim())
}
