use async_trait::async_trait;
use serde::Deserialize;
use tracing::{debug, error, info, instrument};

use super::budget::{estimate_message_tokens, ContextBudget, Message};
use super::preservation::PreservationRules;

#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("LLM call failed: {0}")]
    LlmFailed(String),
    #[error("Failed to parse compactor response: {0}")]
    ParseFailed(String),
    #[error("Compacted output still exceeds target: {actual} tokens vs {target} target")]
    StillOverBudget { actual: usize, target: usize },
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Send a single prompt and get a text response.
    async fn complete(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String, CompactionError>;
}

pub struct SemanticCompactor<C: LlmClient> {
    client: C,
    budget: ContextBudget,
}

#[derive(Debug, Deserialize)]
struct CompactorResponse {
    messages: Vec<CompactorMessage>,
    learnings: Vec<String>,
    preserved_facts: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CompactorMessage {
    role: String,
    content: String,
}

impl<C: LlmClient> SemanticCompactor<C> {
    pub fn new(client: C, budget: ContextBudget) -> Self {
        Self { client, budget }
    }

    /// Run deep compaction on the message list.
    /// Returns a compacted message list targeting budget.target_after_compaction.
    #[instrument(skip_all, fields(message_count = messages.len()))]
    pub async fn compact(
        &self,
        messages: &[Message],
        preservation_rules: &PreservationRules,
    ) -> Result<CompactedContext, CompactionError> {
        info!(
            message_count = messages.len(),
            "starting semantic compaction"
        );
        let preserved_facts = preservation_rules.extract_preserved_facts(messages);
        let target_tokens = self.budget.target_token_count();
        debug!(
            target_tokens,
            preserved_facts = preserved_facts.len(),
            "compaction parameters"
        );

        let (system_prompt, user_prompt) =
            build_compaction_prompt(messages, target_tokens, &preserved_facts);

        debug!("sending compaction prompt to LLM");
        let response = self.client.complete(&system_prompt, &user_prompt).await?;

        let parsed = parse_compactor_response(&response)?;

        let compacted_messages: Vec<Message> = parsed
            .messages
            .into_iter()
            .map(|m| Message {
                role: m.role,
                content: m.content,
            })
            .collect();

        let token_count = estimate_message_tokens(&compacted_messages);

        if token_count > target_tokens {
            error!(
                actual = token_count,
                target = target_tokens,
                "compacted output still over budget"
            );
            return Err(CompactionError::StillOverBudget {
                actual: token_count,
                target: target_tokens,
            });
        }

        info!(
            before_messages = messages.len(),
            after_messages = compacted_messages.len(),
            token_count,
            learnings = parsed.learnings.len(),
            "semantic compaction complete"
        );

        Ok(CompactedContext {
            messages: compacted_messages,
            learnings: parsed.learnings,
            preserved_facts: parsed.preserved_facts,
            token_count,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CompactedContext {
    pub messages: Vec<Message>,
    pub learnings: Vec<String>,
    pub preserved_facts: Vec<String>,
    pub token_count: usize,
}

fn build_compaction_prompt(
    messages: &[Message],
    target_tokens: usize,
    preserved_items: &[String],
) -> (String, String) {
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
        Emit a JSON object with three fields:\n\
        - \"messages\": an array of objects with \"role\" and \"content\" fields\n\
        - \"learnings\": an array of strings extracted from failed attempts\n\
        - \"preserved_facts\": an array of strings (file paths, signatures, etc.)\n\
        \n\
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

    user_prompt.push_str(&format!(
        "## Target\n\nApproximately {target_tokens} tokens.\n"
    ));

    (system_prompt, user_prompt)
}

fn parse_compactor_response(response: &str) -> Result<CompactorResponse, CompactionError> {
    // Try parsing as raw JSON first
    if let Ok(parsed) = serde_json::from_str::<CompactorResponse>(response) {
        return Ok(parsed);
    }

    // Try extracting JSON from markdown code blocks
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
    // Look for ```json ... ``` or ``` ... ```
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
