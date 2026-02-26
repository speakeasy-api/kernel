use std::path::Path;

use sqlx::SqlitePool;
use tracing::{debug, info, instrument, warn};

use super::db;
use super::types::{Mode, ModeOrigin};

#[derive(serde::Deserialize)]
struct TomlMode {
    name: String,
    description: String,
    system_prompt: String,
    default_model: Option<String>,
    allowed_tools: Vec<String>,
    #[serde(default)]
    meta: TomlModeMeta,
}

#[derive(serde::Deserialize)]
struct TomlModeMeta {
    #[serde(default = "default_origin")]
    origin: String,
    #[serde(default = "default_version")]
    version: u32,
}

impl Default for TomlModeMeta {
    fn default() -> Self {
        Self {
            origin: default_origin(),
            version: default_version(),
        }
    }
}

fn default_origin() -> String {
    "user".to_string()
}

fn default_version() -> u32 {
    1
}

/// Parse a TOML mode file into a Mode struct.
#[instrument(skip(content))]
pub fn parse_toml_mode(content: &str) -> Result<Mode, String> {
    let parsed: TomlMode = toml::from_str(content).map_err(|e| format!("TOML parse error: {e}"))?;
    let origin = parsed
        .meta
        .origin
        .parse::<ModeOrigin>()
        .unwrap_or(ModeOrigin::User);

    Ok(Mode {
        name: parsed.name,
        description: parsed.description,
        system_prompt: parsed.system_prompt,
        default_model: parsed.default_model,
        allowed_tools: parsed.allowed_tools,
        created_by: origin,
        version: parsed.meta.version,
    })
}

/// Parse a Markdown mode file (with YAML-like frontmatter) into a Mode struct.
/// Frontmatter is between --- delimiters. Body after frontmatter is the system_prompt.
#[instrument(skip(content))]
pub fn parse_markdown_mode(content: &str) -> Result<Mode, String> {
    let content = content.trim();
    if !content.starts_with("---") {
        return Err("Missing frontmatter delimiter".to_string());
    }

    let after_first = &content[3..];
    let end_idx = after_first
        .find("---")
        .ok_or("Missing closing frontmatter delimiter")?;

    let frontmatter = &after_first[..end_idx];
    let body = after_first[end_idx + 3..].trim();

    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut default_model: Option<String> = None;
    let mut allowed_tools: Vec<String> = Vec::new();
    let mut origin = "user".to_string();
    let mut version: u32 = 1;

    let mut in_list = false;

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // List item continuation (e.g. "  - fs_read")
        if trimmed.starts_with("- ") {
            if in_list {
                allowed_tools.push(trimmed[2..].trim().to_string());
                continue;
            }
        }

        in_list = false;

        if let Some((key, value)) = trimmed.split_once(':') {
            let key = key.trim();
            let value = value.trim();

            match key {
                "name" => name = Some(value.to_string()),
                "description" => description = Some(value.to_string()),
                "default_model" => {
                    if !value.is_empty() {
                        default_model = Some(value.to_string());
                    }
                }
                "origin" => origin = value.to_string(),
                "version" => {
                    version = value
                        .parse()
                        .map_err(|_| format!("invalid version: {value}"))?;
                }
                "allowed_tools" => {
                    if value.is_empty() {
                        // List follows on subsequent lines
                        in_list = true;
                    } else {
                        // Inline comma-separated
                        allowed_tools = value.split(',').map(|s| s.trim().to_string()).collect();
                    }
                }
                _ => {} // ignore unknown keys
            }
        }
    }

    let name = name.ok_or("Missing required field: name")?;
    let description = description.ok_or("Missing required field: description")?;

    let created_by = origin.parse::<ModeOrigin>().unwrap_or(ModeOrigin::User);

    Ok(Mode {
        name,
        description,
        system_prompt: body.to_string(),
        default_model,
        allowed_tools,
        created_by,
        version,
    })
}

/// Parse a mode file, auto-detecting format from extension.
#[instrument(fields(path = %path.display()))]
pub fn parse_mode_file(path: &Path) -> Result<Mode, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;

    match path.extension().and_then(|e| e.to_str()) {
        Some("toml") => parse_toml_mode(&content),
        Some("md") => parse_markdown_mode(&content),
        Some(ext) => Err(format!("unsupported mode file extension: .{ext}")),
        None => Err("mode file has no extension".to_string()),
    }
}

/// Scan the .kernel/modes/ directory and return all parsed modes.
/// Skips files that fail to parse (log a warning but don't fail).
#[instrument(fields(dir = %project_root.display()))]
pub fn scan_mode_files(project_root: &Path) -> Vec<Mode> {
    let modes_dir = project_root.join(".kernel").join("modes");
    info!(dir = %modes_dir.display(), "scanning mode files");
    if !modes_dir.is_dir() {
        debug!("modes directory does not exist, returning empty");
        return Vec::new();
    }

    let entries = match std::fs::read_dir(&modes_dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut modes = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());
        if ext != Some("toml") && ext != Some("md") {
            continue;
        }
        match parse_mode_file(&path) {
            Ok(mode) => modes.push(mode),
            Err(e) => {
                warn!(path = %path.display(), error = %e, "skipping mode file");
            }
        }
    }
    debug!(count = modes.len(), "found mode files");
    modes
}

/// Sync mode files to database. For each file-based mode, upsert into DB.
/// File-based modes take precedence over DB state.
/// Returns the number of modes synced.
#[instrument(skip(pool), fields(project_root = %project_root.display()))]
pub async fn sync_modes_to_db(pool: &SqlitePool, project_root: &Path) -> Result<usize, String> {
    info!("syncing mode files to database");
    let modes = scan_mode_files(project_root);
    let count = modes.len();
    for mode in &modes {
        db::upsert_mode(pool, mode)
            .await
            .map_err(|e| format!("failed to upsert mode '{}': {e}", mode.name))?;
    }
    info!(count, "mode files synced to database");
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_pool;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn parse_toml_full() {
        let content = r#"
name = "database"
description = "Database migration and schema work"
default_model = "claude-sonnet-4-20250514"
allowed_tools = ["fs_read", "fs_write", "glob", "grep", "shell"]
system_prompt = """
You are a database specialist.
"""

[meta]
origin = "user"
version = 1
"#;
        let mode = parse_toml_mode(content).unwrap();
        assert_eq!(mode.name, "database");
        assert_eq!(mode.description, "Database migration and schema work");
        assert_eq!(
            mode.default_model,
            Some("claude-sonnet-4-20250514".to_string())
        );
        assert_eq!(mode.allowed_tools.len(), 5);
        assert_eq!(mode.created_by, ModeOrigin::User);
        assert_eq!(mode.version, 1);
        assert!(mode.system_prompt.contains("database specialist"));
    }

    #[test]
    fn parse_toml_defaults() {
        let content = r#"
name = "minimal"
description = "Minimal mode"
allowed_tools = ["fs_read"]
system_prompt = "You are minimal."
"#;
        let mode = parse_toml_mode(content).unwrap();
        assert_eq!(mode.name, "minimal");
        assert_eq!(mode.default_model, None);
        assert_eq!(mode.created_by, ModeOrigin::User);
        assert_eq!(mode.version, 1);
    }

    #[test]
    fn parse_toml_builtin_origin() {
        let content = r#"
name = "plan"
description = "Planning mode"
allowed_tools = ["fs_read"]
system_prompt = "You plan things."

[meta]
origin = "builtin"
version = 3
"#;
        let mode = parse_toml_mode(content).unwrap();
        assert_eq!(mode.created_by, ModeOrigin::BuiltIn);
        assert_eq!(mode.version, 3);
    }

    #[test]
    fn parse_toml_invalid() {
        let result = parse_toml_mode("not valid toml {{{");
        assert!(result.is_err());
    }

    #[test]
    fn parse_markdown_full() {
        let content = r#"---
name: database
description: Database migration and schema work
default_model: claude-sonnet-4-20250514
allowed_tools:
  - fs_read
  - fs_write
  - glob
  - grep
  - shell
origin: user
version: 1
---

You are a database specialist.

Focus on migrations and schema design."#;
        let mode = parse_markdown_mode(content).unwrap();
        assert_eq!(mode.name, "database");
        assert_eq!(mode.description, "Database migration and schema work");
        assert_eq!(
            mode.default_model,
            Some("claude-sonnet-4-20250514".to_string())
        );
        assert_eq!(mode.allowed_tools.len(), 5);
        assert_eq!(mode.allowed_tools[0], "fs_read");
        assert_eq!(mode.allowed_tools[4], "shell");
        assert_eq!(mode.created_by, ModeOrigin::User);
        assert_eq!(mode.version, 1);
        assert!(mode.system_prompt.contains("database specialist"));
        assert!(mode.system_prompt.contains("schema design"));
    }

    #[test]
    fn parse_markdown_defaults() {
        let content = r#"---
name: minimal
description: Minimal mode
allowed_tools:
  - fs_read
---

You are minimal."#;
        let mode = parse_markdown_mode(content).unwrap();
        assert_eq!(mode.name, "minimal");
        assert_eq!(mode.default_model, None);
        assert_eq!(mode.created_by, ModeOrigin::User);
        assert_eq!(mode.version, 1);
        assert_eq!(mode.system_prompt, "You are minimal.");
    }

    #[test]
    fn parse_markdown_missing_frontmatter() {
        let result = parse_markdown_mode("No frontmatter here.");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Missing frontmatter delimiter"));
    }

    #[test]
    fn parse_markdown_missing_closing_delimiter() {
        let result = parse_markdown_mode("---\nname: broken\n");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Missing closing frontmatter delimiter"));
    }

    #[test]
    fn parse_markdown_missing_name() {
        let content = r#"---
description: No name
allowed_tools:
  - fs_read
---

Body."#;
        let result = parse_markdown_mode(content);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("name"));
    }

    #[test]
    fn parse_mode_file_toml() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.toml");
        fs::write(
            &path,
            r#"
name = "test"
description = "Test mode"
allowed_tools = ["fs_read"]
system_prompt = "You are a test."
"#,
        )
        .unwrap();

        let mode = parse_mode_file(&path).unwrap();
        assert_eq!(mode.name, "test");
    }

    #[test]
    fn parse_mode_file_markdown() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.md");
        fs::write(
            &path,
            r#"---
name: test
description: Test mode
allowed_tools:
  - fs_read
---

You are a test."#,
        )
        .unwrap();

        let mode = parse_mode_file(&path).unwrap();
        assert_eq!(mode.name, "test");
    }

    #[test]
    fn parse_mode_file_unsupported_extension() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.json");
        fs::write(&path, "{}").unwrap();
        assert!(parse_mode_file(&path).is_err());
    }

    #[test]
    fn scan_mode_files_finds_all() {
        let dir = TempDir::new().unwrap();
        let modes_dir = dir.path().join(".kernel").join("modes");
        fs::create_dir_all(&modes_dir).unwrap();

        fs::write(
            modes_dir.join("a.toml"),
            r#"
name = "a"
description = "Mode A"
allowed_tools = ["fs_read"]
system_prompt = "You are A."
"#,
        )
        .unwrap();

        fs::write(
            modes_dir.join("b.md"),
            r#"---
name: b
description: Mode B
allowed_tools:
  - grep
---

You are B."#,
        )
        .unwrap();

        let modes = scan_mode_files(dir.path());
        assert_eq!(modes.len(), 2);
        let names: Vec<&str> = modes.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn scan_mode_files_skips_bad_files() {
        let dir = TempDir::new().unwrap();
        let modes_dir = dir.path().join(".kernel").join("modes");
        fs::create_dir_all(&modes_dir).unwrap();

        // Valid file
        fs::write(
            modes_dir.join("good.toml"),
            r#"
name = "good"
description = "Good mode"
allowed_tools = ["fs_read"]
system_prompt = "You are good."
"#,
        )
        .unwrap();

        // Invalid file
        fs::write(modes_dir.join("bad.toml"), "not valid toml {{{").unwrap();

        // Non-mode file (should be ignored, not cause error)
        fs::write(modes_dir.join("readme.txt"), "ignore me").unwrap();

        let modes = scan_mode_files(dir.path());
        assert_eq!(modes.len(), 1);
        assert_eq!(modes[0].name, "good");
    }

    #[test]
    fn scan_mode_files_missing_dir() {
        let dir = TempDir::new().unwrap();
        let modes = scan_mode_files(dir.path());
        assert!(modes.is_empty());
    }

    #[tokio::test]
    async fn sync_modes_to_db_upserts() {
        let dir = TempDir::new().unwrap();
        let modes_dir = dir.path().join(".kernel").join("modes");
        fs::create_dir_all(&modes_dir).unwrap();

        fs::write(
            modes_dir.join("sync.toml"),
            r#"
name = "sync"
description = "Sync mode"
allowed_tools = ["fs_read", "grep"]
system_prompt = "You sync."
"#,
        )
        .unwrap();

        let pool = test_pool().await;

        let count = sync_modes_to_db(&pool, dir.path()).await.unwrap();
        assert_eq!(count, 1);

        let mode = crate::modes::db::get_mode(&pool, "sync")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mode.description, "Sync mode");
        assert_eq!(mode.allowed_tools, vec!["fs_read", "grep"]);
    }

    #[tokio::test]
    async fn sync_modes_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        let modes_dir = dir.path().join(".kernel").join("modes");
        fs::create_dir_all(&modes_dir).unwrap();

        let pool = test_pool().await;

        // Insert a mode into DB first
        let existing = Mode {
            name: "overwrite".to_string(),
            description: "old description".to_string(),
            system_prompt: "old prompt".to_string(),
            default_model: None,
            allowed_tools: vec!["fs_read".to_string()],
            created_by: ModeOrigin::User,
            version: 1,
        };
        crate::modes::db::create_mode(&pool, &existing)
            .await
            .unwrap();

        // Now create a file that should overwrite it
        fs::write(
            modes_dir.join("overwrite.toml"),
            r#"
name = "overwrite"
description = "new description"
allowed_tools = ["fs_read", "fs_write"]
system_prompt = "new prompt"
"#,
        )
        .unwrap();

        let count = sync_modes_to_db(&pool, dir.path()).await.unwrap();
        assert_eq!(count, 1);

        let mode = crate::modes::db::get_mode(&pool, "overwrite")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mode.description, "new description");
        assert_eq!(mode.system_prompt, "new prompt");
        assert_eq!(mode.allowed_tools, vec!["fs_read", "fs_write"]);
        assert_eq!(mode.version, 2); // upsert increments version
    }

    #[tokio::test]
    async fn sync_empty_dir() {
        let dir = TempDir::new().unwrap();
        let pool = test_pool().await;

        let count = sync_modes_to_db(&pool, dir.path()).await.unwrap();
        assert_eq!(count, 0);
    }
}
