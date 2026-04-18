use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use agentkit::core::{ToolOutput as AkToolOutput, ToolResultPart};
use agentkit::tools::{
    Tool, ToolAnnotations, ToolContext, ToolError, ToolName, ToolRegistry, ToolRequest, ToolResult,
    ToolSpec,
};
use async_trait::async_trait;
use serde::Serialize;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tracing::{debug, warn};

use crate::anthropic::types::ToolDefinition;
use crate::git::diff::LineKind;
use crate::tools::{execute_tool, plans, tool_definitions, FileChangeStatus, ToolOutput};

#[derive(Clone, Serialize)]
struct LlmToolResultEvent {
    id: String,
    content: String,
    is_error: bool,
}

#[derive(Clone, Serialize)]
struct HunkPayload {
    header: String,
    lines: Vec<DiffLinePayload>,
}

#[derive(Clone, Serialize)]
struct DiffLinePayload {
    kind: String,
    content: String,
}

#[derive(Clone, Serialize)]
struct FileChangeEvent {
    tool_use_id: String,
    path: String,
    status: String,
    hunks: Vec<HunkPayload>,
    bytes_written: usize,
    before_content: Option<String>,
    after_content: String,
}

pub struct KernelTool {
    spec: ToolSpec,
    project_path: PathBuf,
    app: AppHandle,
    pool: SqlitePool,
    session_id: String,
}

impl KernelTool {
    fn new(
        def: ToolDefinition,
        project_path: PathBuf,
        app: AppHandle,
        pool: SqlitePool,
        session_id: String,
    ) -> Self {
        let annotations = match def.name.as_str() {
            "fs_read" | "glob" | "grep" | "plan_search" | "read_plan" => {
                ToolAnnotations::read_only()
            }
            "fs_write" | "shell" => ToolAnnotations {
                read_only_hint: false,
                destructive_hint: true,
                idempotent_hint: false,
                needs_approval_hint: false,
                supports_streaming_hint: false,
            },
            _ => ToolAnnotations::new(),
        };
        let spec = ToolSpec::new(ToolName::new(def.name), def.description, def.input_schema)
            .with_annotations(annotations);
        Self {
            spec,
            project_path,
            app,
            pool,
            session_id,
        }
    }
}

#[async_trait]
impl Tool for KernelTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn invoke(
        &self,
        request: ToolRequest,
        _ctx: &mut ToolContext<'_>,
    ) -> Result<ToolResult, ToolError> {
        let name = self.spec.name.0.clone();
        let input = request.input.clone();
        let call_id = request.call_id.0.clone();
        let project = self.project_path.as_path();

        let tc_data = serde_json::json!({"id": call_id, "name": name, "input": input}).to_string();
        let _ = crate::db::queries::insert_event(
            &self.pool,
            &self.session_id,
            None,
            "ToolCall",
            &tc_data,
        )
        .await;

        let started = Instant::now();
        let outcome = if name == "read_plan" {
            exec_read_plan(&self.pool, &self.session_id, project).await
        } else {
            execute_tool(&name, &input, project).await
        };

        if name == "plan_create" {
            if let Ok(ref output) = outcome {
                if let Some(filename) = plans::extract_filename_from_result(output) {
                    let _ = crate::db::queries::attach_plan(
                        &self.pool,
                        &self.session_id,
                        &filename,
                    )
                    .await;
                }
            }
        }

        let (content, is_error, file_change) = match outcome {
            Ok(ToolOutput {
                content,
                file_change,
            }) => (content, false, file_change),
            Err(err) => (err, true, None),
        };

        let tr_data = serde_json::json!({
            "id": call_id,
            "content": content,
            "is_error": is_error,
        })
        .to_string();
        let _ = crate::db::queries::insert_event(
            &self.pool,
            &self.session_id,
            None,
            "ToolResult",
            &tr_data,
        )
        .await;

        let _ = self.app.emit(
            "llm-tool-result",
            LlmToolResultEvent {
                id: call_id.clone(),
                content: content.clone(),
                is_error,
            },
        );

        if let Some(fc) = file_change {
            let status_str = match fc.status {
                FileChangeStatus::Created => "created",
                FileChangeStatus::Modified => "modified",
            };
            let hunks_payload: Vec<HunkPayload> = fc
                .hunks
                .iter()
                .map(|h| HunkPayload {
                    header: h.header.clone(),
                    lines: h
                        .lines
                        .iter()
                        .map(|l| DiffLinePayload {
                            kind: match l.kind {
                                LineKind::Context => "context",
                                LineKind::Add => "add",
                                LineKind::Remove => "remove",
                            }
                            .into(),
                            content: l.content.clone(),
                        })
                        .collect(),
                })
                .collect();

            let fc_event = FileChangeEvent {
                tool_use_id: call_id.clone(),
                path: fc.path.clone(),
                status: status_str.into(),
                hunks: hunks_payload,
                bytes_written: fc.bytes_written,
                before_content: fc.before_content.clone(),
                after_content: fc.after_content.clone(),
            };

            let fc_data = serde_json::to_string(&fc_event).unwrap_or_default();
            let _ = crate::db::queries::insert_event(
                &self.pool,
                &self.session_id,
                None,
                "FileChange",
                &fc_data,
            )
            .await;
            let _ = self.app.emit("file-change", fc_event);
        }

        let result_part = if is_error {
            ToolResultPart::error(request.call_id.clone(), AkToolOutput::Text(content))
        } else {
            ToolResultPart::success(request.call_id.clone(), AkToolOutput::Text(content))
        };
        debug!(tool = %name, elapsed_ms = started.elapsed().as_millis() as u64, is_error, "kernel tool completed");
        Ok(ToolResult::new(result_part).with_duration(started.elapsed()))
    }
}

async fn exec_read_plan(
    pool: &SqlitePool,
    session_id: &str,
    project: &Path,
) -> Result<ToolOutput, String> {
    let filename = match crate::db::queries::get_attached_plan(pool, session_id).await {
        Ok(Some(f)) => f,
        Ok(None) => return Err("No plan is attached to this session.".into()),
        Err(e) => return Err(format!("DB error: {e}")),
    };
    let path = project.join(".kernel/plans").join(&filename);
    match tokio::fs::read_to_string(&path).await {
        Ok(content) => Ok(ToolOutput::text(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let _ = crate::db::queries::detach_plan(pool, session_id).await;
            Err(format!(
                "Attached plan '{filename}' no longer exists. It may have been removed externally."
            ))
        }
        Err(e) => Err(format!("Error reading plan '{filename}': {e}")),
    }
}

/// Build a ToolRegistry with kernel tools for the given allow-list, merged with agentkit-tool-shell.
pub async fn build_registry(
    allowed: &[String],
    project_path: &Path,
    app: AppHandle,
    pool: &SqlitePool,
    session_id: &str,
) -> ToolRegistry {
    let mut effective: Vec<String> = allowed.to_vec();
    let plan_attached = matches!(
        crate::db::queries::get_attached_plan(pool, session_id).await,
        Ok(Some(_))
    );
    let has_plan_family = effective
        .iter()
        .any(|t| t == "plan_create" || t == "plan_search");
    if (plan_attached || has_plan_family) && !effective.iter().any(|t| t == "read_plan") {
        effective.push("read_plan".into());
    }

    let defs = tool_definitions(&effective);
    let mut registry = ToolRegistry::new();
    for def in defs {
        registry.register(KernelTool::new(
            def,
            project_path.to_path_buf(),
            app.clone(),
            pool.clone(),
            session_id.to_string(),
        ));
    }

    let shell_registry = agentkit::tool_shell::registry();
    let mut merged = registry.merge(shell_registry);

    if allowed
        .iter()
        .any(|t| t == "shell" || t == "shell.exec")
    {
        // leave both shell and shell.exec registered
        let _ = &mut merged;
    } else {
        // mode doesn't allow shell — drop agentkit shell.exec by rebuilding without it
        let kept: Vec<Arc<dyn Tool>> = merged
            .tools()
            .into_iter()
            .filter(|t| t.spec().name.0 != "shell.exec")
            .collect();
        let mut filtered = ToolRegistry::new();
        for tool in kept {
            filtered.register_arc(tool);
        }
        merged = filtered;
    }

    if merged.tools().is_empty() {
        warn!("tool registry is empty — LLM will have no tools");
    }
    merged
}
