use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

impl Usage {
    pub fn merge(&mut self, other: &Usage) {
        self.input_tokens = self.input_tokens.max(other.input_tokens);
        self.output_tokens = self.output_tokens.max(other.output_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .max(other.cache_creation_input_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .max(other.cache_read_input_tokens);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_serde_roundtrip() {
        let user = Role::User;
        let json = serde_json::to_string(&user).unwrap();
        assert_eq!(json, r#""user""#);
        let parsed: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Role::User);

        let assistant = Role::Assistant;
        let json = serde_json::to_string(&assistant).unwrap();
        assert_eq!(json, r#""assistant""#);
    }

    #[test]
    fn content_block_text_serde() {
        let block = ContentBlock::Text {
            text: "hello".into(),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "hello");
    }

    #[test]
    fn content_block_tool_use_serde() {
        let block = ContentBlock::ToolUse {
            id: "tu_123".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/tmp/foo"}),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "tool_use");
        assert_eq!(json["id"], "tu_123");
        assert_eq!(json["name"], "read_file");
        assert_eq!(json["input"]["path"], "/tmp/foo");
    }

    #[test]
    fn content_block_tool_result_serde() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "tu_123".into(),
            content: "file contents".into(),
            is_error: false,
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "tool_result");
        assert_eq!(json["tool_use_id"], "tu_123");
        assert!(!json["is_error"].as_bool().unwrap());
    }

    #[test]
    fn message_serde() {
        let msg = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".into(),
            }],
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"][0]["type"], "text");
    }

    #[test]
    fn tool_definition_serde() {
        let tool = ToolDefinition {
            name: "read_file".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        };
        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["name"], "read_file");
    }

    #[test]
    fn usage_defaults_to_zero() {
        let usage: Usage = serde_json::from_str("{}").unwrap();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_creation_input_tokens, 0);
        assert_eq!(usage.cache_read_input_tokens, 0);
    }

    #[test]
    fn usage_deserialize_with_cache_fields() {
        let json = r#"{
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_creation_input_tokens": 10,
            "cache_read_input_tokens": 5
        }"#;
        let usage: Usage = serde_json::from_str(json).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_creation_input_tokens, 10);
        assert_eq!(usage.cache_read_input_tokens, 5);
    }

    #[test]
    fn usage_deserialize_without_cache_fields() {
        let json = r#"{"input_tokens": 200, "output_tokens": 80}"#;
        let usage: Usage = serde_json::from_str(json).unwrap();
        assert_eq!(usage.input_tokens, 200);
        assert_eq!(usage.output_tokens, 80);
        assert_eq!(usage.cache_creation_input_tokens, 0);
        assert_eq!(usage.cache_read_input_tokens, 0);
    }

    #[test]
    fn usage_merge_takes_max() {
        let mut a = Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 10,
            cache_read_input_tokens: 5,
        };
        let b = Usage {
            input_tokens: 150,
            output_tokens: 30,
            cache_creation_input_tokens: 8,
            cache_read_input_tokens: 12,
        };
        a.merge(&b);
        assert_eq!(a.input_tokens, 150);
        assert_eq!(a.output_tokens, 50);
        assert_eq!(a.cache_creation_input_tokens, 10);
        assert_eq!(a.cache_read_input_tokens, 12);
    }
}
