use serde::{Deserialize, Serialize};

/// Where the prompt originated from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PromptSource {
    User,
    AsyncTaskRoot,
}

/// Hints about the project environment to help classification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectContext {
    pub languages: Vec<String>,
    pub frameworks: Vec<String>,
    pub file_structure_hints: Vec<String>,
}

impl Default for ProjectContext {
    fn default() -> Self {
        Self {
            languages: Vec::new(),
            frameworks: Vec::new(),
            file_structure_hints: Vec::new(),
        }
    }
}

// TODO: import from modes module once 05-modes is implemented.
// For now, a placeholder that matches the Mode struct shape for name + description.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeInfo {
    pub name: String,
    pub description: String,
}

// TODO: import from compaction module once 04-compaction is implemented.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactedContext {
    pub messages_summary: String,
    pub learnings: Vec<String>,
    pub preserved_facts: Vec<String>,
    pub token_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterInput {
    pub source: PromptSource,
    pub prompt: String,
    pub available_modes: Vec<ModeInfo>,
    pub conversation_history: CompactedContext,
    pub project_context: ProjectContext,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterOutput {
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub confidence: f32,
}
