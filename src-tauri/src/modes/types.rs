use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mode {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub default_model: Option<String>,
    pub allowed_tools: Vec<String>,
    pub created_by: ModeOrigin,
    pub version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ModeOrigin {
    #[serde(rename = "builtin")]
    BuiltIn,
    #[serde(rename = "ux_agent")]
    UxAgent,
    #[serde(rename = "user")]
    User,
}

impl fmt::Display for ModeOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModeOrigin::BuiltIn => write!(f, "builtin"),
            ModeOrigin::UxAgent => write!(f, "ux_agent"),
            ModeOrigin::User => write!(f, "user"),
        }
    }
}

impl FromStr for ModeOrigin {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "builtin" => Ok(ModeOrigin::BuiltIn),
            "ux_agent" => Ok(ModeOrigin::UxAgent),
            "user" => Ok(ModeOrigin::User),
            _ => Err(format!("unknown mode origin: {s}")),
        }
    }
}

pub const READ_ONLY_TOOLS: &[&str] = &["fs_read", "glob", "grep"];
pub const READ_WRITE_TOOLS: &[&str] = &["fs_read", "fs_write", "glob", "grep"];
pub const FULL_TOOLS: &[&str] = &["fs_read", "fs_write", "glob", "grep", "shell", "git"];
pub const WEB_TOOLS: &[&str] = &["web_search", "web_fetch"];
pub const GIT_TOOLS: &[&str] = &["git"];

pub fn combine_tool_sets(sets: &[&[&str]]) -> Vec<String> {
    let mut tools: Vec<String> = sets
        .iter()
        .flat_map(|s| s.iter().map(|t| t.to_string()))
        .collect();
    tools.sort();
    tools.dedup();
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_origin_display() {
        assert_eq!(ModeOrigin::BuiltIn.to_string(), "builtin");
        assert_eq!(ModeOrigin::UxAgent.to_string(), "ux_agent");
        assert_eq!(ModeOrigin::User.to_string(), "user");
    }

    #[test]
    fn mode_origin_from_str() {
        assert_eq!(
            "builtin".parse::<ModeOrigin>().unwrap(),
            ModeOrigin::BuiltIn
        );
        assert_eq!(
            "ux_agent".parse::<ModeOrigin>().unwrap(),
            ModeOrigin::UxAgent
        );
        assert_eq!("user".parse::<ModeOrigin>().unwrap(), ModeOrigin::User);
        assert!("unknown".parse::<ModeOrigin>().is_err());
    }

    #[test]
    fn mode_origin_serde_roundtrip() {
        let origin = ModeOrigin::BuiltIn;
        let json = serde_json::to_string(&origin).unwrap();
        assert_eq!(json, r#""builtin""#);
        let parsed: ModeOrigin = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ModeOrigin::BuiltIn);

        let origin = ModeOrigin::UxAgent;
        let json = serde_json::to_string(&origin).unwrap();
        assert_eq!(json, r#""ux_agent""#);
        let parsed: ModeOrigin = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ModeOrigin::UxAgent);
    }

    #[test]
    fn mode_serde_roundtrip() {
        let mode = Mode {
            name: "test".into(),
            description: "A test mode".into(),
            system_prompt: "You are a test assistant.".into(),
            default_model: Some("claude-sonnet-4-6".into()),
            allowed_tools: vec!["fs_read".into(), "grep".into()],
            created_by: ModeOrigin::User,
            version: 1,
        };
        let json = serde_json::to_string(&mode).unwrap();
        let parsed: Mode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, mode);
    }

    #[test]
    fn combine_tool_sets_deduplicates_and_sorts() {
        let combined = combine_tool_sets(&[READ_ONLY_TOOLS, GIT_TOOLS]);
        assert_eq!(combined, vec!["fs_read", "git", "glob", "grep"]);
    }

    #[test]
    fn combine_tool_sets_handles_overlap() {
        let combined = combine_tool_sets(&[READ_ONLY_TOOLS, FULL_TOOLS]);
        assert_eq!(
            combined,
            vec!["fs_read", "fs_write", "git", "glob", "grep", "shell"]
        );
    }

    #[test]
    fn combine_tool_sets_empty() {
        let combined = combine_tool_sets(&[]);
        assert!(combined.is_empty());
    }
}
