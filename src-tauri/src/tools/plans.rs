use rand::Rng;
use serde_json::Value;
use std::path::Path;
use tracing::{debug, instrument};

use super::ToolOutput;

/// Convert a title to a URL-friendly slug.
pub fn slugify(title: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse consecutive hyphens and trim leading/trailing hyphens
    let mut result = String::new();
    let mut prev_hyphen = true; // start true to trim leading
    for c in slug.chars() {
        if c == '-' {
            if !prev_hyphen {
                result.push('-');
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }
    // Trim trailing hyphen
    if result.ends_with('-') {
        result.pop();
    }
    result
}

/// Generate a 4-character random alphanumeric postfix.
pub fn random_postfix() -> String {
    let mut rng = rand::rng();
    (0..4)
        .map(|_| {
            let idx = rng.random_range(0..36u8);
            if idx < 10 {
                (b'0' + idx) as char
            } else {
                (b'a' + idx - 10) as char
            }
        })
        .collect()
}

const PLANS_DIR: &str = ".kernel/plans";
const MAX_RETRIES: usize = 5;

/// Execute `plan_create` tool: create a new plan file in `.kernel/plans/`.
#[instrument(skip(input, project_path))]
pub async fn exec_plan_create(input: &Value, project_path: &Path) -> Result<ToolOutput, String> {
    let title = input["title"]
        .as_str()
        .ok_or_else(|| "missing 'title'".to_string())?;

    let slug = slugify(title);
    if slug.is_empty() {
        return Err("title produces an empty slug".to_string());
    }

    let default_content = format!("# {title}\n");
    let content = input["content"].as_str().unwrap_or(&default_content);

    let plans_dir = project_path.join(PLANS_DIR);
    tokio::fs::create_dir_all(&plans_dir)
        .await
        .map_err(|e| format!("failed to create plans directory: {e}"))?;

    // Try to create with unique postfix, retry on collision
    for _ in 0..MAX_RETRIES {
        let postfix = random_postfix();
        let filename = format!("{slug}-{postfix}.md");
        let file_path = plans_dir.join(&filename);

        if file_path.exists() {
            continue;
        }

        tokio::fs::write(&file_path, content)
            .await
            .map_err(|e| format!("failed to write plan file: {e}"))?;

        let rel_path = format!("{PLANS_DIR}/{filename}");
        debug!(filename = %filename, "created plan file");
        return Ok(ToolOutput::text(format!(
            "Created plan: {filename}\nPath: {rel_path}"
        )));
    }

    Err("failed to generate unique filename after retries".to_string())
}

/// Extract the filename from a `plan_create` result.
pub fn extract_filename_from_result(output: &ToolOutput) -> Option<String> {
    output
        .content
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("Created plan: "))
        .map(|s| s.to_string())
}

/// Execute `plan_search` tool: find plans in `.kernel/plans/`.
#[instrument(skip(input, project_path))]
pub async fn exec_plan_search(input: &Value, project_path: &Path) -> Result<ToolOutput, String> {
    let query = input["query"].as_str().unwrap_or("");
    let plans_dir = project_path.join(PLANS_DIR);

    let mut entries = match tokio::fs::read_dir(&plans_dir).await {
        Ok(entries) => entries,
        Err(_) => return Ok(ToolOutput::text("No plans found.".into())),
    };

    let query_lower = query.to_lowercase();
    let mut results: Vec<String> = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let filename = entry.file_name().to_string_lossy().to_string();

        if query.is_empty() {
            // No query — list all plans with first-line preview
            let preview = read_first_line(&path).await;
            results.push(format!("{filename}  {preview}"));
        } else {
            // Match against filename and content
            let filename_matches = filename.to_lowercase().contains(&query_lower);
            let content = tokio::fs::read_to_string(&path).await.unwrap_or_default();
            let content_matches = content.to_lowercase().contains(&query_lower);

            if filename_matches || content_matches {
                let preview = content
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string();
                results.push(format!("{filename}  {preview}"));
            }
        }
    }

    if results.is_empty() {
        Ok(ToolOutput::text("No plans found.".into()))
    } else {
        results.sort();
        Ok(ToolOutput::text(results.join("\n")))
    }
}

async fn read_first_line(path: &Path) -> String {
    tokio::fs::read_to_string(path)
        .await
        .ok()
        .and_then(|c| c.lines().next().map(|s| s.to_string()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Add New Theme"), "add-new-theme");
    }

    #[test]
    fn slugify_special_chars() {
        assert_eq!(slugify("Hello, World! #2"), "hello-world-2");
    }

    #[test]
    fn slugify_consecutive_hyphens() {
        assert_eq!(slugify("a---b"), "a-b");
    }

    #[test]
    fn slugify_leading_trailing() {
        assert_eq!(slugify("  hello  "), "hello");
    }

    #[test]
    fn slugify_empty_after_strip() {
        assert_eq!(slugify("---"), "");
    }

    #[test]
    fn random_postfix_length_and_chars() {
        for _ in 0..20 {
            let p = random_postfix();
            assert_eq!(p.len(), 4);
            assert!(p.chars().all(|c| c.is_ascii_alphanumeric()));
        }
    }

    #[tokio::test]
    async fn plan_create_basic() {
        let dir = TempDir::new().unwrap();
        let input = json!({"title": "Add auth feature"});
        let result = exec_plan_create(&input, dir.path()).await.unwrap();
        assert!(result.content.starts_with("Created plan: add-auth-feature-"));
        assert!(result.content.contains(".kernel/plans/"));

        // Verify file exists
        let filename = extract_filename_from_result(&result).unwrap();
        let path = dir.path().join(PLANS_DIR).join(&filename);
        assert!(path.exists());

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "# Add auth feature\n");
    }

    #[tokio::test]
    async fn plan_create_with_content() {
        let dir = TempDir::new().unwrap();
        let input = json!({"title": "My Plan", "content": "custom content here"});
        let result = exec_plan_create(&input, dir.path()).await.unwrap();

        let filename = extract_filename_from_result(&result).unwrap();
        let path = dir.path().join(PLANS_DIR).join(&filename);
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "custom content here");
    }

    #[tokio::test]
    async fn plan_create_empty_slug_error() {
        let dir = TempDir::new().unwrap();
        let input = json!({"title": "---"});
        let result = exec_plan_create(&input, dir.path()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty slug"));
    }

    #[tokio::test]
    async fn plan_search_empty_dir() {
        let dir = TempDir::new().unwrap();
        let input = json!({});
        let result = exec_plan_search(&input, dir.path()).await.unwrap();
        assert_eq!(result.content, "No plans found.");
    }

    #[tokio::test]
    async fn plan_search_lists_all() {
        let dir = TempDir::new().unwrap();
        let plans_dir = dir.path().join(PLANS_DIR);
        std::fs::create_dir_all(&plans_dir).unwrap();
        std::fs::write(plans_dir.join("plan-a-1234.md"), "# Plan A\ndetails").unwrap();
        std::fs::write(plans_dir.join("plan-b-5678.md"), "# Plan B\nstuff").unwrap();

        let input = json!({});
        let result = exec_plan_search(&input, dir.path()).await.unwrap();
        assert!(result.content.contains("plan-a-1234.md"));
        assert!(result.content.contains("plan-b-5678.md"));
    }

    #[tokio::test]
    async fn plan_search_with_query_matches_filename() {
        let dir = TempDir::new().unwrap();
        let plans_dir = dir.path().join(PLANS_DIR);
        std::fs::create_dir_all(&plans_dir).unwrap();
        std::fs::write(plans_dir.join("auth-feature-ab12.md"), "# Auth\n").unwrap();
        std::fs::write(plans_dir.join("theme-update-cd34.md"), "# Theme\n").unwrap();

        let input = json!({"query": "auth"});
        let result = exec_plan_search(&input, dir.path()).await.unwrap();
        assert!(result.content.contains("auth-feature-ab12.md"));
        assert!(!result.content.contains("theme-update-cd34.md"));
    }

    #[tokio::test]
    async fn plan_search_with_query_matches_content() {
        let dir = TempDir::new().unwrap();
        let plans_dir = dir.path().join(PLANS_DIR);
        std::fs::create_dir_all(&plans_dir).unwrap();
        std::fs::write(
            plans_dir.join("plan-a-1234.md"),
            "# Plan A\nImplement JWT tokens",
        )
        .unwrap();
        std::fs::write(
            plans_dir.join("plan-b-5678.md"),
            "# Plan B\nAdd dark theme",
        )
        .unwrap();

        let input = json!({"query": "JWT"});
        let result = exec_plan_search(&input, dir.path()).await.unwrap();
        assert!(result.content.contains("plan-a-1234.md"));
        assert!(!result.content.contains("plan-b-5678.md"));
    }

    #[tokio::test]
    async fn plan_search_no_matches() {
        let dir = TempDir::new().unwrap();
        let plans_dir = dir.path().join(PLANS_DIR);
        std::fs::create_dir_all(&plans_dir).unwrap();
        std::fs::write(plans_dir.join("plan-a-1234.md"), "# Plan A\n").unwrap();

        let input = json!({"query": "nonexistent"});
        let result = exec_plan_search(&input, dir.path()).await.unwrap();
        assert_eq!(result.content, "No plans found.");
    }

    #[test]
    fn extract_filename_works() {
        let output = ToolOutput::text("Created plan: my-plan-ab12.md\nPath: .kernel/plans/my-plan-ab12.md".into());
        assert_eq!(
            extract_filename_from_result(&output),
            Some("my-plan-ab12.md".to_string())
        );
    }

    #[test]
    fn extract_filename_none_for_bad_output() {
        let output = ToolOutput::text("something else".into());
        assert_eq!(extract_filename_from_result(&output), None);
    }
}
