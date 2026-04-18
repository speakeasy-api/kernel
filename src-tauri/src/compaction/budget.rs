use serde::{Deserialize, Serialize};

/// Compact representation of a single conversation turn used by the
/// compaction prompt builder. The kernel converts agentkit `Item`s and
/// anthropic `Message`s into this lossy textual form purely for prompting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_snippet: Option<String>,
}

/// Estimate token count for a string. Uses the standard ~4 chars-per-token
/// heuristic shared across kernel components — switch to a real tokenizer
/// later if precision matters.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    (text.len() + 3) / 4
}

/// Estimate total token count for a slice of messages, including a small
/// per-message overhead for role / framing tokens.
pub fn estimate_message_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|m| estimate_tokens(&m.content) + 4)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_empty_is_zero() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn estimate_tokens_short_string() {
        // 4 chars / 4 = 1 token
        assert_eq!(estimate_tokens("abcd"), 1);
        // 5 chars rounds up
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn estimate_message_tokens_includes_overhead() {
        let messages = vec![
            Message {
                role: "user".into(),
                content: "abcd".into(),
                pinned: false,
                context_snippet: None,
            },
            Message {
                role: "assistant".into(),
                content: "abcdefgh".into(),
                pinned: false,
                context_snippet: None,
            },
        ];
        // 1 + 4 + 2 + 4 = 11
        assert_eq!(estimate_message_tokens(&messages), 11);
    }
}
