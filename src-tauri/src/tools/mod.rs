use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::{debug, error, info, instrument};

use crate::anthropic::types::ToolDefinition;
use crate::git::diff::{DiffLine, Hunk, LineKind};

/// Build tool definitions for the subset of tools an agent is allowed to use.
pub fn tool_definitions(allowed: &[String]) -> Vec<ToolDefinition> {
    all_tool_definitions()
        .into_iter()
        .filter(|t| allowed.iter().any(|a| a == &t.name))
        .collect()
}

fn all_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "fs_read".into(),
            description: "Read the contents of a file with line numbers. Path is relative to the project root. For large files, use offset/limit to read specific line ranges."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path relative to the project root"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "1-indexed start line (optional)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Number of lines to return (optional)"
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: "fs_write".into(),
            description:
                "Create or overwrite a file. Creates parent directories if needed. Path is relative to the project root."
                    .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path relative to the project root"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write"
                    }
                },
                "required": ["path", "content"]
            }),
        },
        ToolDefinition {
            name: "glob".into(),
            description:
                "Find files matching a glob pattern. Returns matching paths one per line. Use '*' to list a directory, '**/*.ext' for recursive search."
                    .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern (e.g. '**/*.rs', 'src/*.ts', '*')"
                    }
                },
                "required": ["pattern"]
            }),
        },
        ToolDefinition {
            name: "grep".into(),
            description:
                "Search file contents for a regex pattern. Returns matching lines grouped by file with line numbers. Use before/after for context lines around matches."
                    .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory or file to search in (relative, defaults to '.')"
                    },
                    "include": {
                        "type": "string",
                        "description": "Glob to filter files (e.g. '*.rs', '*.ts')"
                    },
                    "before": {
                        "type": "integer",
                        "description": "Context lines before each match (default 0)"
                    },
                    "after": {
                        "type": "integer",
                        "description": "Context lines after each match (default 0)"
                    }
                },
                "required": ["pattern"]
            }),
        },
        ToolDefinition {
            name: "shell".into(),
            description: "Execute a shell command in the project directory. Returns stdout/stderr."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to run"
                    }
                },
                "required": ["command"]
            }),
        },
    ]
}

// ── Tool output types ─────────────────────────────────────────────────────

/// Result of a tool execution — carries the LLM-facing text plus optional
/// structured file-change data for the frontend.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// The text content returned to the LLM (unchanged from previous behaviour).
    pub content: String,
    /// Structured file-change data (only produced by fs_write).
    pub file_change: Option<FileChange>,
}

impl ToolOutput {
    pub fn text(content: String) -> Self {
        Self {
            content,
            file_change: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub status: FileChangeStatus,
    /// Content before the write (`None` for newly created files).
    pub before_content: Option<String>,
    /// Content after the write.
    pub after_content: String,
    /// Structured hunk data.
    pub hunks: Vec<Hunk>,
    /// Total bytes written.
    pub bytes_written: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileChangeStatus {
    Created,
    Modified,
}

/// Result of attempting to revert a file write.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum RevertResult {
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "conflict")]
    Conflict {
        expected_hash: String,
        actual_hash: String,
    },
    #[serde(rename = "not_found")]
    NotFound,
    #[serde(rename = "error")]
    Error { message: String },
}

const MAX_OUTPUT: usize = 30_000;

fn truncate(s: String) -> String {
    if s.len() > MAX_OUTPUT {
        format!(
            "{}...\n\n(truncated, {} bytes total)",
            &s[..MAX_OUTPUT],
            s.len()
        )
    } else {
        s
    }
}

/// Execute a tool by name. Returns Ok(ToolOutput) or Err(error_message).
#[instrument(skip(input, project_path), fields(tool = name))]
pub async fn execute_tool(
    name: &str,
    input: &Value,
    project_path: &Path,
) -> Result<ToolOutput, String> {
    info!(tool = name, "executing tool");
    let result = match name {
        "fs_read" => exec_fs_read(input, project_path)
            .await
            .map(ToolOutput::text),
        "fs_write" => exec_fs_write(input, project_path).await,
        "glob" => exec_glob(input, project_path).await.map(ToolOutput::text),
        "grep" => exec_grep(input, project_path).await.map(ToolOutput::text),
        "shell" => exec_shell(input, project_path).await.map(ToolOutput::text),
        other => {
            error!(tool = other, "unknown tool");
            Err(format!("Unknown tool: {other}"))
        }
    };
    match &result {
        Ok(output) => debug!(
            tool = name,
            bytes = output.content.len(),
            has_file_change = output.file_change.is_some(),
            "tool completed"
        ),
        Err(err) => error!(tool = name, error = %err, "tool failed"),
    }
    result
}

fn resolve_path(project: &Path, rel: &str) -> PathBuf {
    if Path::new(rel).is_absolute() {
        PathBuf::from(rel)
    } else {
        project.join(rel)
    }
}

/// Format lines with line numbers, matching grep output style.
fn format_lines_numbered(lines: &[&str], start: usize) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let line_num = start + i;
        out.push_str(&format!("{line_num:>5}| {line}\n"));
    }
    out
}

#[instrument(skip(input, project))]
async fn exec_fs_read(input: &Value, project: &Path) -> Result<String, String> {
    let path = input["path"]
        .as_str()
        .ok_or_else(|| "missing 'path'".to_string())?;
    let resolved = resolve_path(project, path);
    let content = tokio::fs::read_to_string(&resolved)
        .await
        .map_err(|e| format!("Error reading {path}: {e}"))?;

    let offset = input["offset"].as_u64();
    let limit = input["limit"].as_u64();

    let lines: Vec<&str> = content.lines().collect();

    if offset.is_some() || limit.is_some() {
        // Line-range mode
        let start = offset.unwrap_or(1).max(1) as usize;
        let count = limit.unwrap_or(lines.len() as u64) as usize;
        let start_idx = start.saturating_sub(1); // 1-indexed to 0-indexed
        let end_idx = (start_idx + count).min(lines.len());

        if start_idx >= lines.len() {
            return Ok(format!(
                "(file has {} lines, offset {start} is past end)",
                lines.len()
            ));
        }

        let slice = &lines[start_idx..end_idx];
        let formatted = format_lines_numbered(slice, start);
        Ok(truncate(format!(
            "{path} (lines {start}-{}, {} total)\n{formatted}",
            start_idx + slice.len(),
            lines.len()
        )))
    } else {
        // Whole-file mode — error if too large
        if content.len() > MAX_OUTPUT {
            return Err(format!(
                "File too large ({} bytes, {} lines). Use offset/limit to read specific sections.",
                content.len(),
                lines.len()
            ));
        }
        let formatted = format_lines_numbered(&lines, 1);
        Ok(format!("{path} ({} lines)\n{formatted}", lines.len()))
    }
}

#[instrument(skip(input, project))]
async fn exec_fs_write(input: &Value, project: &Path) -> Result<ToolOutput, String> {
    let path = input["path"]
        .as_str()
        .ok_or_else(|| "missing 'path'".to_string())?;
    let content = input["content"]
        .as_str()
        .ok_or_else(|| "missing 'content'".to_string())?;
    let resolved = resolve_path(project, path);

    // Read existing content before writing (None for new files).
    let before = tokio::fs::read_to_string(&resolved).await.ok();

    if let Some(parent) = resolved.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Cannot create dirs: {e}"))?;
    }
    tokio::fs::write(&resolved, content)
        .await
        .map_err(|e| format!("Error writing {path}: {e}"))?;

    let old_text = before.as_deref().unwrap_or("");
    let hunks = build_hunks(old_text, content);
    let status = if before.is_some() {
        FileChangeStatus::Modified
    } else {
        FileChangeStatus::Created
    };

    Ok(ToolOutput {
        content: format!("Wrote {} bytes to {path}", content.len()),
        file_change: Some(FileChange {
            path: path.to_string(),
            status,
            before_content: before,
            after_content: content.to_string(),
            hunks,
            bytes_written: content.len(),
        }),
    })
}

/// Build structured hunks from before/after content using the `similar` crate.
fn build_hunks(old: &str, new: &str) -> Vec<Hunk> {
    let diff = TextDiff::from_lines(old, new);
    let mut hunks = Vec::new();

    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        let header = hunk.header().to_string();
        let lines = hunk
            .iter_changes()
            .map(|change| {
                let kind = match change.tag() {
                    ChangeTag::Equal => LineKind::Context,
                    ChangeTag::Insert => LineKind::Add,
                    ChangeTag::Delete => LineKind::Remove,
                };
                DiffLine {
                    kind,
                    content: change.value().trim_end_matches('\n').to_string(),
                }
            })
            .collect();
        hunks.push(Hunk { header, lines });
    }

    hunks
}

/// Revert a file write. Checks for conflicts before applying.
pub async fn revert_file_write(
    project_path: &Path,
    rel_path: &str,
    before_content: Option<&str>,
    after_content: &str,
    force: bool,
) -> RevertResult {
    let resolved = resolve_path(project_path, rel_path);

    // Read current file content
    let current = match tokio::fs::read_to_string(&resolved).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return RevertResult::NotFound;
        }
        Err(e) => {
            return RevertResult::Error {
                message: format!("Failed to read file: {e}"),
            };
        }
    };

    // Conflict check: current content should match what we wrote
    if !force && current != after_content {
        return RevertResult::Conflict {
            expected_hash: short_hash(after_content),
            actual_hash: short_hash(&current),
        };
    }

    // Apply revert
    match before_content {
        Some(content) => {
            if let Err(e) = tokio::fs::write(&resolved, content).await {
                return RevertResult::Error {
                    message: format!("Failed to write: {e}"),
                };
            }
        }
        None => {
            // File was newly created — delete it
            if let Err(e) = tokio::fs::remove_file(&resolved).await {
                return RevertResult::Error {
                    message: format!("Failed to delete: {e}"),
                };
            }
        }
    }

    RevertResult::Success
}

/// Simple 8-char hash for conflict diagnostics (not cryptographic).
fn short_hash(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Tool command timeout.
const CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Build a gitignore matcher from `.gitignore` + `.kernelignore`.
/// `.kernelignore` is loaded second so its rules take precedence.
fn load_gitignore(project: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(project);
    // .gitignore first
    let _ = builder.add(project.join(".gitignore"));
    // .kernelignore takes precedence (loaded second, can negate with !)
    let _ = builder.add(project.join(".kernelignore"));
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

#[instrument(skip(input, project))]
async fn exec_glob(input: &Value, project: &Path) -> Result<String, String> {
    let pattern = input["pattern"]
        .as_str()
        .ok_or_else(|| "missing 'pattern'".to_string())?;
    let full = project.join(pattern).to_string_lossy().to_string();
    let gi = load_gitignore(project);
    let entries: Vec<String> = glob::glob(&full)
        .map_err(|e| format!("Invalid pattern: {e}"))?
        .filter_map(|e| e.ok())
        .filter(|p| {
            let is_dir = p.is_dir();
            !gi.matched_path_or_any_parents(p, is_dir).is_ignore()
        })
        .filter_map(|p| {
            p.strip_prefix(project)
                .ok()
                .map(|r| r.to_string_lossy().to_string())
        })
        .collect();
    if entries.is_empty() {
        Ok("No files matched.".into())
    } else {
        Ok(truncate(entries.join("\n")))
    }
}

/// Native grep implementation using `regex` + `ignore::WalkBuilder` + `crossbeam-channel`.
fn exec_grep_sync(
    pattern: &str,
    search_path: &Path,
    project: &Path,
    include: Option<&str>,
    before: usize,
    after: usize,
) -> Result<String, String> {
    let re = Regex::new(pattern).map_err(|e| format!("Invalid regex: {e}"))?;

    let mut walk = WalkBuilder::new(search_path);
    walk.hidden(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .add_custom_ignore_filename(".kernelignore");

    // Also load .gitignore from the project root explicitly,
    // so it works even outside a git repo (e.g. in tests).
    let gitignore_path = project.join(".gitignore");
    if gitignore_path.exists() {
        walk.add_ignore(&gitignore_path);
    }

    if let Some(inc) = include {
        let mut overrides = OverrideBuilder::new(search_path);
        overrides
            .add(inc)
            .map_err(|e| format!("Invalid include glob: {e}"))?;
        let built = overrides
            .build()
            .map_err(|e| format!("Failed to build glob: {e}"))?;
        walk.overrides(built);
    }

    let (tx, rx) = crossbeam_channel::bounded::<PathBuf>(256);

    // Spawn walker thread
    let walker = walk.build();
    let sender = std::thread::spawn(move || {
        for entry in walker {
            if let Ok(entry) = entry {
                if entry.file_type().map_or(false, |ft| ft.is_file()) {
                    if tx.send(entry.into_path()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Collect results sorted by path for determinism
    let mut results: BTreeMap<String, String> = BTreeMap::new();
    let mut total_len: usize = 0;

    for file_path in rx {
        let content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(_) => continue, // skip binary/unreadable files
        };

        let lines: Vec<&str> = content.lines().collect();
        let mut match_indices: Vec<usize> = Vec::new();

        for (i, line) in lines.iter().enumerate() {
            if re.is_match(line) {
                match_indices.push(i);
            }
        }

        if match_indices.is_empty() {
            continue;
        }

        // Build context ranges (merge overlapping)
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for &idx in &match_indices {
            let start = idx.saturating_sub(before);
            let end = (idx + after + 1).min(lines.len());
            if let Some(last) = ranges.last_mut() {
                if start <= last.1 {
                    last.1 = end; // merge overlapping
                    continue;
                }
            }
            ranges.push((start, end));
        }

        // Format output for this file
        let rel_path = file_path
            .strip_prefix(project)
            .unwrap_or(&file_path)
            .to_string_lossy()
            .to_string();

        let mut file_output = String::new();
        for (range_idx, &(start, end)) in ranges.iter().enumerate() {
            if range_idx > 0 {
                file_output.push_str("  --\n");
            }
            for i in start..end {
                file_output.push_str(&format!("{:>5}| {}\n", i + 1, lines[i]));
            }
        }

        total_len += rel_path.len() + 1 + file_output.len();
        results.insert(rel_path, file_output);

        if total_len > MAX_OUTPUT * 2 {
            break; // stop early if we have way too much output
        }
    }

    let _ = sender.join();

    if results.is_empty() {
        return Ok("No matches.".into());
    }

    let mut output = String::new();
    for (path, content) in &results {
        output.push_str(path);
        output.push('\n');
        output.push_str(content);
    }

    Ok(truncate(output.trim_end().to_string()))
}

#[instrument(skip(input, project))]
async fn exec_grep(input: &Value, project: &Path) -> Result<String, String> {
    let pattern = input["pattern"]
        .as_str()
        .ok_or_else(|| "missing 'pattern'".to_string())?
        .to_string();
    let search_path_str = input["path"].as_str().unwrap_or(".");
    let include = input["include"].as_str().map(|s| s.to_string());
    let before = input["before"].as_u64().unwrap_or(0) as usize;
    let after = input["after"].as_u64().unwrap_or(0) as usize;

    let search_path = resolve_path(project, search_path_str);
    let project = project.to_path_buf();

    tokio::task::spawn_blocking(move || {
        exec_grep_sync(
            &pattern,
            &search_path,
            &project,
            include.as_deref(),
            before,
            after,
        )
    })
    .await
    .map_err(|e| format!("grep task failed: {e}"))?
}

#[instrument(skip(input, project))]
async fn exec_shell(input: &Value, project: &Path) -> Result<String, String> {
    let command = input["command"]
        .as_str()
        .ok_or_else(|| "missing 'command'".to_string())?;
    let output = tokio::time::timeout(
        CMD_TIMEOUT,
        Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(project)
            .output(),
    )
    .await
    .map_err(|_| "Command timed out (30s)".to_string())?
    .map_err(|e| format!("Command failed: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut result = stdout.to_string();
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&stderr);
    }
    if !output.status.success() {
        Err(if result.is_empty() {
            format!("Exit code {}", output.status)
        } else {
            truncate(result)
        })
    } else {
        Ok(if result.is_empty() {
            "(no output)".into()
        } else {
            truncate(result)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn tool_definitions_filters_by_allowed() {
        let allowed = vec!["fs_read".to_string(), "grep".to_string()];
        let defs = tool_definitions(&allowed);
        assert_eq!(defs.len(), 2);
        assert!(defs.iter().any(|t| t.name == "fs_read"));
        assert!(defs.iter().any(|t| t.name == "grep"));
    }

    #[test]
    fn tool_definitions_returns_empty_for_unknown() {
        let allowed = vec!["web_search".to_string()];
        let defs = tool_definitions(&allowed);
        assert!(defs.is_empty());
    }

    #[test]
    fn tool_definitions_all_have_schemas() {
        let allowed: Vec<String> = vec!["fs_read", "fs_write", "glob", "grep", "shell"]
            .into_iter()
            .map(String::from)
            .collect();
        let defs = tool_definitions(&allowed);
        assert_eq!(defs.len(), 5);
        for def in &defs {
            assert!(def.input_schema["type"] == "object");
            assert!(def.input_schema["required"].is_array());
        }
    }

    #[test]
    fn truncate_leaves_short_strings() {
        let s = "hello".to_string();
        assert_eq!(truncate(s.clone()), s);
    }

    #[test]
    fn truncate_cuts_long_strings() {
        let s = "a".repeat(MAX_OUTPUT + 100);
        let result = truncate(s.clone());
        assert!(result.len() < s.len());
        assert!(result.contains("truncated"));
    }

    // ---- fs_read tests ----

    #[tokio::test]
    async fn fs_read_whole_file() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("hello.txt"),
            "line one\nline two\nline three\n",
        )
        .unwrap();

        let input = json!({"path": "hello.txt"});
        let result = exec_fs_read(&input, dir.path()).await.unwrap();
        assert!(result.contains("hello.txt (3 lines)"));
        assert!(result.contains("    1| line one"));
        assert!(result.contains("    2| line two"));
        assert!(result.contains("    3| line three"));
    }

    #[tokio::test]
    async fn fs_read_with_offset_limit() {
        let dir = TempDir::new().unwrap();
        let content = (1..=20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(dir.path().join("big.txt"), &content).unwrap();

        let input = json!({"path": "big.txt", "offset": 5, "limit": 3});
        let result = exec_fs_read(&input, dir.path()).await.unwrap();
        assert!(result.contains("lines 5-7"));
        assert!(result.contains("    5| line 5"));
        assert!(result.contains("    6| line 6"));
        assert!(result.contains("    7| line 7"));
        assert!(!result.contains("line 4"));
        assert!(!result.contains("line 8"));
    }

    #[tokio::test]
    async fn fs_read_error_on_oversized_without_range() {
        let dir = TempDir::new().unwrap();
        let content = "x".repeat(MAX_OUTPUT + 1);
        fs::write(dir.path().join("huge.txt"), &content).unwrap();

        let input = json!({"path": "huge.txt"});
        let result = exec_fs_read(&input, dir.path()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("offset/limit"));
    }

    #[tokio::test]
    async fn fs_read_oversized_with_range_succeeds() {
        let dir = TempDir::new().unwrap();
        let content = (1..=1000)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(dir.path().join("big.txt"), &content).unwrap();

        let input = json!({"path": "big.txt", "offset": 1, "limit": 5});
        let result = exec_fs_read(&input, dir.path()).await.unwrap();
        assert!(result.contains("    1| line 1"));
        assert!(result.contains("    5| line 5"));
    }

    #[tokio::test]
    async fn fs_read_offset_past_end() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("small.txt"), "one\ntwo\n").unwrap();

        let input = json!({"path": "small.txt", "offset": 100});
        let result = exec_fs_read(&input, dir.path()).await.unwrap();
        assert!(result.contains("past end"));
    }

    // ---- grep tests ----

    fn setup_grep_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/main.rs"),
            "fn main() {\n    println!(\"hello\");\n    let x = 42;\n    println!(\"world\");\n}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("readme.txt"),
            "This is a readme.\nIt has println in it.\n",
        )
        .unwrap();
        // Create a .gitignore to test filtering
        fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();
        fs::create_dir_all(dir.path().join("ignored")).unwrap();
        fs::write(
            dir.path().join("ignored/secret.rs"),
            "fn secret() { println!(\"secret\"); }\n",
        )
        .unwrap();
        dir
    }

    #[tokio::test]
    async fn grep_basic_match() {
        let dir = setup_grep_dir();
        let input = json!({"pattern": "println"});
        let result = exec_grep(&input, dir.path()).await.unwrap();
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("println"));
        // Should not include ignored files
        assert!(!result.contains("secret"));
    }

    #[tokio::test]
    async fn grep_with_include_filter() {
        let dir = setup_grep_dir();
        let input = json!({"pattern": "println", "include": "*.rs"});
        let result = exec_grep(&input, dir.path()).await.unwrap();
        assert!(result.contains("src/main.rs"));
        assert!(!result.contains("readme.txt"));
    }

    #[tokio::test]
    async fn grep_with_context_lines() {
        let dir = setup_grep_dir();
        let input = json!({"pattern": "x = 42", "before": 1, "after": 1});
        let result = exec_grep(&input, dir.path()).await.unwrap();
        // Should include line before (println("hello")) and after (println("world"))
        assert!(result.contains("hello"));
        assert!(result.contains("42"));
        assert!(result.contains("world"));
    }

    #[tokio::test]
    async fn grep_no_matches() {
        let dir = setup_grep_dir();
        let input = json!({"pattern": "nonexistent_string_xyz"});
        let result = exec_grep(&input, dir.path()).await.unwrap();
        assert_eq!(result, "No matches.");
    }

    #[tokio::test]
    async fn grep_gitignore_filtering() {
        let dir = setup_grep_dir();
        let input = json!({"pattern": "secret"});
        let result = exec_grep(&input, dir.path()).await.unwrap();
        // ignored/ directory should be excluded
        assert!(!result.contains("ignored/secret.rs"));
    }

    #[tokio::test]
    async fn grep_invalid_regex() {
        let dir = setup_grep_dir();
        let input = json!({"pattern": "[invalid"});
        let result = exec_grep(&input, dir.path()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid regex"));
    }

    #[tokio::test]
    async fn grep_line_numbers_format() {
        let dir = setup_grep_dir();
        let input = json!({"pattern": "pub fn add"});
        let result = exec_grep(&input, dir.path()).await.unwrap();
        // Should have line-numbered output
        assert!(result.contains("1| pub fn add"));
    }

    #[tokio::test]
    async fn grep_subdirectory_search() {
        let dir = setup_grep_dir();
        let input = json!({"pattern": "fn", "path": "src"});
        let result = exec_grep(&input, dir.path()).await.unwrap();
        assert!(result.contains("main"));
        assert!(!result.contains("readme"));
    }
}
