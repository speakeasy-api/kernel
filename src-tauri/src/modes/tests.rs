use super::*;
use rusqlite::Connection;
use std::fs as stdfs;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn setup_test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    db::ensure_modes_table(&conn).unwrap();
    conn
}

fn test_mode(name: &str) -> Mode {
    Mode {
        name: name.to_string(),
        description: format!("Test mode: {name}"),
        system_prompt: "You are a test mode.".to_string(),
        default_model: None,
        allowed_tools: vec!["fs_read".to_string()],
        created_by: ModeOrigin::User,
        version: 1,
    }
}

fn test_mode_with_origin(name: &str, origin: ModeOrigin) -> Mode {
    let mut mode = test_mode(name);
    mode.created_by = origin;
    mode
}

// ---------------------------------------------------------------------------
// 1. Type tests
// ---------------------------------------------------------------------------

#[test]
fn mode_origin_display_and_parse() {
    for (variant, expected) in [
        (ModeOrigin::BuiltIn, "builtin"),
        (ModeOrigin::UxAgent, "ux_agent"),
        (ModeOrigin::User, "user"),
    ] {
        assert_eq!(variant.to_string(), expected);
        assert_eq!(expected.parse::<ModeOrigin>().unwrap(), variant);
    }
    assert!("invalid".parse::<ModeOrigin>().is_err());
}

#[test]
fn mode_origin_serde_roundtrip() {
    let json = serde_json::to_string(&ModeOrigin::BuiltIn).unwrap();
    assert_eq!(json, r#""builtin""#);
    let parsed: ModeOrigin = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, ModeOrigin::BuiltIn);

    let json = serde_json::to_string(&ModeOrigin::UxAgent).unwrap();
    assert_eq!(json, r#""ux_agent""#);
    let parsed: ModeOrigin = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, ModeOrigin::UxAgent);
}

#[test]
fn mode_serde_roundtrip() {
    let mode = Mode {
        name: "roundtrip".into(),
        description: "Round-trip test".into(),
        system_prompt: "You are a roundtrip test.".into(),
        default_model: Some("claude-sonnet-4-6".into()),
        allowed_tools: vec!["fs_read".into(), "grep".into()],
        created_by: ModeOrigin::User,
        version: 3,
    };
    let json = serde_json::to_string(&mode).unwrap();
    let parsed: Mode = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, mode);
}

// ---------------------------------------------------------------------------
// 2. Tool permission set tests
// ---------------------------------------------------------------------------

#[test]
fn tool_permission_sets_contents() {
    assert_eq!(READ_ONLY_TOOLS, &["fs_read", "glob", "grep"]);
    assert_eq!(READ_WRITE_TOOLS, &["fs_read", "fs_write", "glob", "grep"]);
    assert_eq!(
        FULL_TOOLS,
        &["fs_read", "fs_write", "glob", "grep", "shell", "git"]
    );
    assert_eq!(WEB_TOOLS, &["web_search", "web_fetch"]);
    assert_eq!(GIT_TOOLS, &["git"]);
}

#[test]
fn combine_tool_sets_union() {
    let combined = combine_tool_sets(&[READ_ONLY_TOOLS, GIT_TOOLS]);
    assert_eq!(combined, vec!["fs_read", "git", "glob", "grep"]);
}

#[test]
fn combine_tool_sets_dedup() {
    let combined = combine_tool_sets(&[FULL_TOOLS, READ_ONLY_TOOLS]);
    // FULL_TOOLS is a superset of READ_ONLY_TOOLS, so no extra entries
    assert_eq!(combined.len(), FULL_TOOLS.len());
    // Verify no duplicates
    let mut sorted = combined.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), combined.len());
}

#[test]
fn combine_tool_sets_empty_input() {
    let combined = combine_tool_sets(&[]);
    assert!(combined.is_empty());
}

// ---------------------------------------------------------------------------
// 3. Built-in mode tests
// ---------------------------------------------------------------------------

#[test]
fn builtin_modes_count() {
    assert_eq!(builtin::builtin_modes().len(), 6);
}

#[test]
fn builtin_modes_expected_names() {
    let modes = builtin::builtin_modes();
    let mut names: Vec<&str> = modes.iter().map(|m| m.name.as_str()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "debug",
            "general",
            "implement",
            "plan",
            "research",
            "review"
        ]
    );
}

#[test]
fn builtin_modes_all_builtin_origin() {
    for mode in builtin::builtin_modes() {
        assert_eq!(
            mode.created_by,
            ModeOrigin::BuiltIn,
            "mode '{}' should be BuiltIn",
            mode.name
        );
    }
}

#[test]
fn builtin_modes_all_version_one() {
    for mode in builtin::builtin_modes() {
        assert_eq!(mode.version, 1, "mode '{}' should be version 1", mode.name);
    }
}

#[test]
fn builtin_modes_no_default_model() {
    for mode in builtin::builtin_modes() {
        assert_eq!(
            mode.default_model, None,
            "mode '{}' should have no default model",
            mode.name
        );
    }
}

#[test]
fn builtin_modes_all_have_substantial_system_prompts() {
    for mode in builtin::builtin_modes() {
        assert!(
            mode.system_prompt.len() >= 200,
            "mode '{}' system_prompt is only {} chars",
            mode.name,
            mode.system_prompt.len()
        );
    }
}

#[test]
fn plan_mode_tools_are_read_only() {
    let modes = builtin::builtin_modes();
    let plan = modes.iter().find(|m| m.name == "plan").unwrap();
    let expected: Vec<String> = READ_ONLY_TOOLS.iter().map(|s| s.to_string()).collect();
    assert_eq!(plan.allowed_tools, expected);
}

#[test]
fn review_mode_tools_include_git() {
    let modes = builtin::builtin_modes();
    let review = modes.iter().find(|m| m.name == "review").unwrap();
    let expected = combine_tool_sets(&[READ_ONLY_TOOLS, GIT_TOOLS]);
    assert_eq!(review.allowed_tools, expected);
    assert!(review.allowed_tools.contains(&"git".to_string()));
}

#[test]
fn research_mode_tools_include_web() {
    let modes = builtin::builtin_modes();
    let research = modes.iter().find(|m| m.name == "research").unwrap();
    let expected = combine_tool_sets(&[READ_ONLY_TOOLS, WEB_TOOLS]);
    assert_eq!(research.allowed_tools, expected);
    assert!(research.allowed_tools.contains(&"web_search".to_string()));
    assert!(research.allowed_tools.contains(&"web_fetch".to_string()));
}

#[test]
fn implement_mode_has_full_tools() {
    let modes = builtin::builtin_modes();
    let implement = modes.iter().find(|m| m.name == "implement").unwrap();
    let expected: Vec<String> = FULL_TOOLS.iter().map(|s| s.to_string()).collect();
    assert_eq!(implement.allowed_tools, expected);
}

// ---------------------------------------------------------------------------
// 4. CRUD tests
// ---------------------------------------------------------------------------

#[test]
fn create_and_get_mode() {
    let conn = setup_test_db();
    let mode = test_mode("crud_test");
    db::create_mode(&conn, &mode).unwrap();

    let fetched = db::get_mode(&conn, "crud_test").unwrap().unwrap();
    assert_eq!(fetched.name, "crud_test");
    assert_eq!(fetched.description, "Test mode: crud_test");
    assert_eq!(fetched.system_prompt, "You are a test mode.");
    assert_eq!(fetched.default_model, None);
    assert_eq!(fetched.allowed_tools, vec!["fs_read"]);
    assert_eq!(fetched.created_by, ModeOrigin::User);
    assert_eq!(fetched.version, 1);
}

#[test]
fn get_nonexistent_mode() {
    let conn = setup_test_db();
    assert!(db::get_mode(&conn, "no_such_mode").unwrap().is_none());
}

#[test]
fn list_modes_empty() {
    let conn = setup_test_db();
    let modes = db::list_modes(&conn).unwrap();
    assert!(modes.is_empty());
}

#[test]
fn list_modes_returns_all() {
    let conn = setup_test_db();
    db::create_mode(&conn, &test_mode("x")).unwrap();
    db::create_mode(&conn, &test_mode("y")).unwrap();
    db::create_mode(&conn, &test_mode("z")).unwrap();

    let modes = db::list_modes(&conn).unwrap();
    assert_eq!(modes.len(), 3);
}

#[test]
fn create_duplicate_mode_fails() {
    let conn = setup_test_db();
    let mode = test_mode("dup");
    db::create_mode(&conn, &mode).unwrap();
    assert!(db::create_mode(&conn, &mode).is_err());
}

#[test]
fn update_mode_increments_version() {
    let conn = setup_test_db();
    let mode = test_mode("versioned");
    db::create_mode(&conn, &mode).unwrap();

    let mut updated = mode.clone();
    updated.description = "updated".into();
    db::update_mode(&conn, "versioned", &updated).unwrap();

    let fetched = db::get_mode(&conn, "versioned").unwrap().unwrap();
    assert_eq!(fetched.version, 2);
}

#[test]
fn update_mode_changes_fields() {
    let conn = setup_test_db();
    let mode = test_mode("mutable");
    db::create_mode(&conn, &mode).unwrap();

    let mut changed = mode.clone();
    changed.description = "new desc".into();
    changed.system_prompt = "new prompt".into();
    db::update_mode(&conn, "mutable", &changed).unwrap();

    let fetched = db::get_mode(&conn, "mutable").unwrap().unwrap();
    assert_eq!(fetched.description, "new desc");
    assert_eq!(fetched.system_prompt, "new prompt");
}

#[test]
fn delete_builtin_mode_fails() {
    let conn = setup_test_db();
    let mode = test_mode_with_origin("protected", ModeOrigin::BuiltIn);
    db::create_mode(&conn, &mode).unwrap();

    let err = db::delete_mode(&conn, "protected").unwrap_err();
    assert!(matches!(err, db::ModeError::CannotDeleteBuiltin(_)));
    assert!(db::get_mode(&conn, "protected").unwrap().is_some());
}

#[test]
fn delete_user_mode_succeeds() {
    let conn = setup_test_db();
    let mode = test_mode_with_origin("removable", ModeOrigin::User);
    db::create_mode(&conn, &mode).unwrap();

    assert!(db::delete_mode(&conn, "removable").unwrap());
    assert!(db::get_mode(&conn, "removable").unwrap().is_none());
}

#[test]
fn delete_nonexistent_mode() {
    let conn = setup_test_db();
    assert!(!db::delete_mode(&conn, "ghost").unwrap());
}

#[test]
fn upsert_inserts_new_mode() {
    let conn = setup_test_db();
    let mode = test_mode("upsert_new");
    db::upsert_mode(&conn, &mode).unwrap();

    let fetched = db::get_mode(&conn, "upsert_new").unwrap().unwrap();
    assert_eq!(fetched.version, 1);
    assert_eq!(fetched.description, "Test mode: upsert_new");
}

#[test]
fn upsert_updates_existing_mode() {
    let conn = setup_test_db();
    let mode = test_mode("upsert_upd");
    db::create_mode(&conn, &mode).unwrap();

    let mut changed = mode.clone();
    changed.description = "upserted".into();
    db::upsert_mode(&conn, &changed).unwrap();

    let fetched = db::get_mode(&conn, "upsert_upd").unwrap().unwrap();
    assert_eq!(fetched.description, "upserted");
    assert_eq!(fetched.version, 2);
}

#[test]
fn allowed_tools_json_roundtrip() {
    let conn = setup_test_db();
    let mut mode = test_mode("tools_rt");
    mode.allowed_tools = vec![
        "fs_read".into(),
        "fs_write".into(),
        "glob".into(),
        "grep".into(),
        "shell".into(),
        "git".into(),
    ];
    db::create_mode(&conn, &mode).unwrap();

    let fetched = db::get_mode(&conn, "tools_rt").unwrap().unwrap();
    assert_eq!(fetched.allowed_tools, mode.allowed_tools);
}

// ---------------------------------------------------------------------------
// 5. File system parsing tests
// ---------------------------------------------------------------------------

#[test]
fn parse_toml_mode() {
    let content = r#"
name = "database"
description = "Database work"
default_model = "claude-sonnet-4-6"
allowed_tools = ["fs_read", "fs_write", "shell"]
system_prompt = "You are a database specialist."

[meta]
origin = "user"
version = 2
"#;
    let mode = fs::parse_toml_mode(content).unwrap();
    assert_eq!(mode.name, "database");
    assert_eq!(mode.description, "Database work");
    assert_eq!(mode.default_model, Some("claude-sonnet-4-6".to_string()));
    assert_eq!(mode.allowed_tools, vec!["fs_read", "fs_write", "shell"]);
    assert_eq!(mode.created_by, ModeOrigin::User);
    assert_eq!(mode.version, 2);
}

#[test]
fn parse_toml_mode_minimal() {
    let content = r#"
name = "minimal"
description = "Minimal"
allowed_tools = ["fs_read"]
system_prompt = "You are minimal."
"#;
    let mode = fs::parse_toml_mode(content).unwrap();
    assert_eq!(mode.default_model, None);
    assert_eq!(mode.created_by, ModeOrigin::User);
    assert_eq!(mode.version, 1);
}

#[test]
fn parse_toml_mode_invalid() {
    assert!(fs::parse_toml_mode("not valid {{{").is_err());
}

#[test]
fn parse_markdown_mode() {
    let content = r#"---
name: reviewer
description: Code review
default_model: claude-sonnet-4-6
allowed_tools:
  - fs_read
  - git
origin: user
version: 1
---

You are a code reviewer.

Focus on correctness and security."#;
    let mode = fs::parse_markdown_mode(content).unwrap();
    assert_eq!(mode.name, "reviewer");
    assert_eq!(mode.description, "Code review");
    assert_eq!(mode.default_model, Some("claude-sonnet-4-6".to_string()));
    assert_eq!(mode.allowed_tools, vec!["fs_read", "git"]);
    assert_eq!(mode.created_by, ModeOrigin::User);
    assert!(mode.system_prompt.contains("code reviewer"));
    assert!(mode.system_prompt.contains("security"));
}

#[test]
fn parse_markdown_mode_no_frontmatter() {
    let result = fs::parse_markdown_mode("Just some text, no delimiters.");
    assert!(result.is_err());
}

#[test]
fn parse_markdown_mode_empty_body() {
    let content = r#"---
name: empty
description: Empty body
allowed_tools:
  - fs_read
---
"#;
    let mode = fs::parse_markdown_mode(content).unwrap();
    assert_eq!(mode.system_prompt, "");
}

#[test]
fn parse_markdown_mode_inline_tools() {
    let content = r#"---
name: inline
description: Inline tools
allowed_tools: fs_read, grep, glob
---

Prompt."#;
    let mode = fs::parse_markdown_mode(content).unwrap();
    assert_eq!(mode.allowed_tools, vec!["fs_read", "grep", "glob"]);
}

#[test]
fn scan_mode_files_mixed_formats() {
    let dir = TempDir::new().unwrap();
    let modes_dir = dir.path().join(".kernel").join("modes");
    stdfs::create_dir_all(&modes_dir).unwrap();

    stdfs::write(
        modes_dir.join("a.toml"),
        r#"
name = "a"
description = "Mode A"
allowed_tools = ["fs_read"]
system_prompt = "A."
"#,
    )
    .unwrap();

    stdfs::write(
        modes_dir.join("b.md"),
        r#"---
name: b
description: Mode B
allowed_tools:
  - grep
---

B."#,
    )
    .unwrap();

    let modes = fs::scan_mode_files(dir.path());
    assert_eq!(modes.len(), 2);
    let names: Vec<&str> = modes.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"a"));
    assert!(names.contains(&"b"));
}

#[test]
fn scan_mode_files_skips_invalid() {
    let dir = TempDir::new().unwrap();
    let modes_dir = dir.path().join(".kernel").join("modes");
    stdfs::create_dir_all(&modes_dir).unwrap();

    stdfs::write(
        modes_dir.join("good.toml"),
        r#"
name = "good"
description = "Good"
allowed_tools = ["fs_read"]
system_prompt = "Good."
"#,
    )
    .unwrap();

    stdfs::write(modes_dir.join("bad.toml"), "not valid {{{").unwrap();

    let modes = fs::scan_mode_files(dir.path());
    assert_eq!(modes.len(), 1);
    assert_eq!(modes[0].name, "good");
}

#[test]
fn scan_mode_files_missing_directory() {
    let dir = TempDir::new().unwrap();
    let modes = fs::scan_mode_files(dir.path());
    assert!(modes.is_empty());
}

#[test]
fn scan_mode_files_ignores_other_extensions() {
    let dir = TempDir::new().unwrap();
    let modes_dir = dir.path().join(".kernel").join("modes");
    stdfs::create_dir_all(&modes_dir).unwrap();

    stdfs::write(modes_dir.join("notes.txt"), "ignore me").unwrap();
    stdfs::write(modes_dir.join("data.json"), "{}").unwrap();

    stdfs::write(
        modes_dir.join("real.toml"),
        r#"
name = "real"
description = "Real mode"
allowed_tools = ["fs_read"]
system_prompt = "Real."
"#,
    )
    .unwrap();

    let modes = fs::scan_mode_files(dir.path());
    assert_eq!(modes.len(), 1);
    assert_eq!(modes[0].name, "real");
}

#[test]
fn sync_modes_to_db() {
    let dir = TempDir::new().unwrap();
    let modes_dir = dir.path().join(".kernel").join("modes");
    stdfs::create_dir_all(&modes_dir).unwrap();

    stdfs::write(
        modes_dir.join("alpha.toml"),
        r#"
name = "alpha"
description = "Alpha"
allowed_tools = ["fs_read"]
system_prompt = "Alpha."
"#,
    )
    .unwrap();

    stdfs::write(
        modes_dir.join("beta.md"),
        r#"---
name: beta
description: Beta
allowed_tools:
  - grep
---

Beta."#,
    )
    .unwrap();

    let conn = setup_test_db();
    let count = fs::sync_modes_to_db(&conn, dir.path()).unwrap();
    assert_eq!(count, 2);

    let all = db::list_modes(&conn).unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn sync_modes_overrides_db() {
    let conn = setup_test_db();
    let existing = Mode {
        name: "override_me".to_string(),
        description: "old".to_string(),
        system_prompt: "old prompt".to_string(),
        default_model: None,
        allowed_tools: vec!["fs_read".to_string()],
        created_by: ModeOrigin::User,
        version: 1,
    };
    db::create_mode(&conn, &existing).unwrap();

    let dir = TempDir::new().unwrap();
    let modes_dir = dir.path().join(".kernel").join("modes");
    stdfs::create_dir_all(&modes_dir).unwrap();

    stdfs::write(
        modes_dir.join("override_me.toml"),
        r#"
name = "override_me"
description = "new"
allowed_tools = ["fs_read", "fs_write"]
system_prompt = "new prompt"
"#,
    )
    .unwrap();

    fs::sync_modes_to_db(&conn, dir.path()).unwrap();

    let fetched = db::get_mode(&conn, "override_me").unwrap().unwrap();
    assert_eq!(fetched.description, "new");
    assert_eq!(fetched.system_prompt, "new prompt");
    assert_eq!(fetched.allowed_tools, vec!["fs_read", "fs_write"]);
    assert_eq!(fetched.version, 2);
}

// ---------------------------------------------------------------------------
// 6. Cross-module integration tests
// ---------------------------------------------------------------------------

#[test]
fn builtin_modes_persist_to_db() {
    let conn = setup_test_db();
    for mode in builtin::builtin_modes() {
        db::create_mode(&conn, &mode).unwrap();
    }

    let all = db::list_modes(&conn).unwrap();
    assert_eq!(all.len(), 6);

    let plan = db::get_mode(&conn, "plan").unwrap().unwrap();
    assert_eq!(plan.created_by, ModeOrigin::BuiltIn);
    assert!(!plan.system_prompt.is_empty());
}

#[test]
fn builtin_mode_cannot_be_deleted_from_db() {
    let conn = setup_test_db();
    for mode in builtin::builtin_modes() {
        db::create_mode(&conn, &mode).unwrap();
    }

    let err = db::delete_mode(&conn, "plan").unwrap_err();
    assert!(matches!(err, db::ModeError::CannotDeleteBuiltin(_)));
    assert!(db::get_mode(&conn, "plan").unwrap().is_some());
}

#[test]
fn file_mode_overrides_builtin_in_db_via_sync() {
    let conn = setup_test_db();
    // Seed DB with builtin plan mode
    let builtins = builtin::builtin_modes();
    let plan = builtins.iter().find(|m| m.name == "plan").unwrap();
    db::create_mode(&conn, plan).unwrap();

    // Create a file-based plan mode that should override
    let dir = TempDir::new().unwrap();
    let modes_dir = dir.path().join(".kernel").join("modes");
    stdfs::create_dir_all(&modes_dir).unwrap();

    stdfs::write(
        modes_dir.join("plan.toml"),
        r#"
name = "plan"
description = "Custom plan mode"
allowed_tools = ["fs_read", "glob", "grep"]
system_prompt = "You are a custom planner."

[meta]
origin = "user"
version = 1
"#,
    )
    .unwrap();

    fs::sync_modes_to_db(&conn, dir.path()).unwrap();

    let fetched = db::get_mode(&conn, "plan").unwrap().unwrap();
    assert_eq!(fetched.description, "Custom plan mode");
    assert_eq!(fetched.system_prompt, "You are a custom planner.");
    assert_eq!(fetched.version, 2);
}

#[test]
fn end_to_end_mode_lifecycle() {
    let conn = setup_test_db();

    // 1. Start with empty DB
    assert!(db::list_modes(&conn).unwrap().is_empty());

    // 2. Create a user mode
    let mode = test_mode("workflow");
    db::create_mode(&conn, &mode).unwrap();
    assert_eq!(db::list_modes(&conn).unwrap().len(), 1);

    // 3. Update it
    let mut updated = mode.clone();
    updated.description = "Updated workflow".into();
    updated.system_prompt = "Updated prompt.".into();
    db::update_mode(&conn, "workflow", &updated).unwrap();

    let fetched = db::get_mode(&conn, "workflow").unwrap().unwrap();
    assert_eq!(fetched.version, 2);
    assert_eq!(fetched.description, "Updated workflow");

    // 4. Upsert overwrites again
    let mut v3 = updated.clone();
    v3.description = "V3 workflow".into();
    db::upsert_mode(&conn, &v3).unwrap();

    let fetched = db::get_mode(&conn, "workflow").unwrap().unwrap();
    assert_eq!(fetched.version, 3);
    assert_eq!(fetched.description, "V3 workflow");

    // 5. Delete user mode
    assert!(db::delete_mode(&conn, "workflow").unwrap());
    assert!(db::get_mode(&conn, "workflow").unwrap().is_none());
}
