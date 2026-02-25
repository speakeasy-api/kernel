use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, error, info};

const KERNEL_DIR: &str = ".kernel";
const WORKTREES_DIR: &str = "worktrees";
const WORKTREE_META_DIR: &str = "worktree-meta";
const BRANCH_PREFIX: &str = "kernel/";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub task_id: Option<String>,
    pub base_ref: String,
    pub base_commit: String,
    pub merge_target_ref: String,
    pub created_at: String,
}

#[derive(Debug)]
pub enum WorktreeError {
    Git(String),
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for WorktreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(msg) => write!(f, "git error: {msg}"),
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Json(err) => write!(f, "json error: {err}"),
        }
    }
}

impl std::error::Error for WorktreeError {}

impl From<std::io::Error> for WorktreeError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for WorktreeError {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

fn git(project_root: &Path, args: &[&str]) -> Result<String, WorktreeError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(WorktreeError::Git(stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn worktrees_dir(project_root: &Path) -> PathBuf {
    project_root.join(KERNEL_DIR).join(WORKTREES_DIR)
}

fn meta_dir(project_root: &Path) -> PathBuf {
    project_root.join(KERNEL_DIR).join(WORKTREE_META_DIR)
}

fn slug_from_branch(branch: &str) -> &str {
    branch.strip_prefix(BRANCH_PREFIX).unwrap_or(branch)
}

/// Creates a git worktree under `.kernel/worktrees/<branch_name>/` with a new
/// branch `kernel/<branch_name>` based on `base_ref`. The resolved commit SHA
/// of `base_ref` is recorded as `base_commit`.
pub fn create_worktree(
    project_root: &Path,
    task_id: Option<&str>,
    branch_name: &str,
    base_ref: &str,
    merge_target_ref: &str,
) -> Result<Worktree, WorktreeError> {
    info!(
        branch = %branch_name,
        base_ref = %base_ref,
        merge_target = %merge_target_ref,
        task_id = ?task_id,
        "creating worktree"
    );
    let full_branch = format!("{BRANCH_PREFIX}{branch_name}");
    let worktree_path = worktrees_dir(project_root).join(branch_name);

    std::fs::create_dir_all(worktrees_dir(project_root))?;

    let base_commit = git(project_root, &["rev-parse", base_ref])?
        .trim()
        .to_string();

    let worktree_path_str = worktree_path.to_string_lossy().to_string();
    git(
        project_root,
        &[
            "worktree",
            "add",
            &worktree_path_str,
            "-b",
            &full_branch,
            base_ref,
        ],
    )?;

    let created_at = chrono::Utc::now().to_rfc3339();

    let worktree = Worktree {
        path: worktree_path,
        branch: full_branch,
        task_id: task_id.map(String::from),
        base_ref: base_ref.to_string(),
        base_commit,
        merge_target_ref: merge_target_ref.to_string(),
        created_at,
    };

    // Persist metadata so list_worktrees can return full Worktree structs
    let meta = meta_dir(project_root);
    std::fs::create_dir_all(&meta)?;
    let meta_file = meta.join(format!("{branch_name}.json"));
    std::fs::write(meta_file, serde_json::to_string_pretty(&worktree)?)?;

    info!(path = %worktree.path.display(), branch = %worktree.branch, "worktree created");
    Ok(worktree)
}

/// Removes a git worktree and deletes its branch.
pub fn remove_worktree(project_root: &Path, branch_name: &str) -> Result<(), WorktreeError> {
    info!(branch = %branch_name, "removing worktree");
    let full_branch = format!("{BRANCH_PREFIX}{branch_name}");
    let worktree_path = worktrees_dir(project_root).join(branch_name);
    let worktree_path_str = worktree_path.to_string_lossy().to_string();

    git(project_root, &["worktree", "remove", &worktree_path_str]).map_err(|e| {
        error!(branch = %branch_name, error = %e, "failed to remove worktree");
        e
    })?;
    git(project_root, &["branch", "-d", &full_branch]).map_err(|e| {
        error!(branch = %full_branch, error = %e, "failed to delete branch");
        e
    })?;

    let meta_file = meta_dir(project_root).join(format!("{branch_name}.json"));
    if meta_file.exists() {
        std::fs::remove_file(meta_file)?;
    }

    info!(branch = %branch_name, "worktree removed");
    Ok(())
}

/// Lists all kernel-managed worktrees (those under `.kernel/worktrees/`).
/// Parses `git worktree list --porcelain` and enriches results with
/// persisted metadata when available.
pub fn list_worktrees(project_root: &Path) -> Result<Vec<Worktree>, WorktreeError> {
    debug!("listing worktrees");
    let output = git(project_root, &["worktree", "list", "--porcelain"])?;
    // Canonicalize to handle symlinks (e.g. macOS /tmp -> /private/tmp)
    let kernel_wt_dir = worktrees_dir(project_root)
        .canonicalize()
        .unwrap_or_else(|_| worktrees_dir(project_root));
    let mut worktrees = Vec::new();

    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;

    for line in output.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path_str));
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            current_branch = Some(
                branch_ref
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch_ref)
                    .to_string(),
            );
        } else if line.is_empty() {
            if let (Some(path), Some(branch)) = (current_path.take(), current_branch.take()) {
                if path.starts_with(&kernel_wt_dir) {
                    worktrees.push(load_worktree(project_root, path, branch)?);
                }
            }
            current_path = None;
            current_branch = None;
        }
    }

    // Handle trailing entry (porcelain output may not end with a blank line)
    if let (Some(path), Some(branch)) = (current_path, current_branch) {
        if path.starts_with(&kernel_wt_dir) {
            worktrees.push(load_worktree(project_root, path, branch)?);
        }
    }

    debug!(count = worktrees.len(), "worktrees listed");
    Ok(worktrees)
}

/// Stub: looks up a worktree by task_id. Requires DB integration.
pub fn worktree_for_task(
    _project_root: &Path,
    _task_id: &str,
) -> Result<Option<Worktree>, WorktreeError> {
    Ok(None)
}

fn load_worktree(
    project_root: &Path,
    path: PathBuf,
    branch: String,
) -> Result<Worktree, WorktreeError> {
    let slug = slug_from_branch(&branch);
    let meta_file = meta_dir(project_root).join(format!("{slug}.json"));

    if meta_file.exists() {
        let json = std::fs::read_to_string(&meta_file)?;
        Ok(serde_json::from_str(&json)?)
    } else {
        Ok(Worktree {
            path,
            branch,
            task_id: None,
            base_ref: String::new(),
            base_commit: String::new(),
            merge_target_ref: String::new(),
            created_at: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Creates a temporary git repo with an initial commit.
    fn init_test_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path();

        Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(path)
            .output()
            .unwrap();

        // Create an initial commit so HEAD exists
        std::fs::write(path.join("README.md"), "# test\n").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(path)
            .output()
            .unwrap();

        tmp
    }

    #[test]
    fn create_worktree_creates_directory_and_branch() {
        let tmp = init_test_repo();
        let root = tmp.path();

        let wt = create_worktree(root, Some("task-1"), "task-auth", "HEAD", "main").unwrap();

        assert!(wt.path.exists(), "worktree directory should exist");
        assert_eq!(wt.branch, "kernel/task-auth");
        assert_eq!(wt.task_id, Some("task-1".to_string()));
        assert_eq!(wt.base_ref, "HEAD");
        assert!(!wt.base_commit.is_empty(), "base_commit should be resolved");
        assert_eq!(wt.merge_target_ref, "main");
        assert!(!wt.created_at.is_empty());

        // Verify the git branch exists
        let branches = git(root, &["branch", "--list", "kernel/task-auth"]).unwrap();
        assert!(branches.contains("kernel/task-auth"));
    }

    #[test]
    fn create_worktree_resolves_base_commit() {
        let tmp = init_test_repo();
        let root = tmp.path();

        let head_sha = git(root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let wt = create_worktree(root, None, "task-resolve", "HEAD", "main").unwrap();

        assert_eq!(wt.base_commit, head_sha);
    }

    #[test]
    fn create_worktree_stores_metadata() {
        let tmp = init_test_repo();
        let root = tmp.path();

        create_worktree(root, Some("t-42"), "task-meta", "HEAD", "main").unwrap();

        let meta_file = meta_dir(root).join("task-meta.json");
        assert!(meta_file.exists(), "metadata file should exist");

        let json = std::fs::read_to_string(&meta_file).unwrap();
        let loaded: Worktree = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.branch, "kernel/task-meta");
        assert_eq!(loaded.task_id, Some("t-42".to_string()));
    }

    #[test]
    fn list_worktrees_returns_kernel_worktrees() {
        let tmp = init_test_repo();
        let root = tmp.path();

        create_worktree(root, Some("t-1"), "task-list-a", "HEAD", "main").unwrap();
        create_worktree(root, Some("t-2"), "task-list-b", "HEAD", "main").unwrap();

        let worktrees = list_worktrees(root).unwrap();
        assert_eq!(worktrees.len(), 2);

        let branches: Vec<&str> = worktrees.iter().map(|w| w.branch.as_str()).collect();
        assert!(branches.contains(&"kernel/task-list-a"));
        assert!(branches.contains(&"kernel/task-list-b"));
    }

    #[test]
    fn list_worktrees_excludes_main_worktree() {
        let tmp = init_test_repo();
        let root = tmp.path();

        // With no kernel worktrees, list should be empty
        let worktrees = list_worktrees(root).unwrap();
        assert!(worktrees.is_empty());
    }

    #[test]
    fn list_worktrees_loads_metadata() {
        let tmp = init_test_repo();
        let root = tmp.path();

        create_worktree(root, Some("t-meta"), "task-enriched", "HEAD", "main").unwrap();

        let worktrees = list_worktrees(root).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].task_id, Some("t-meta".to_string()));
        assert_eq!(worktrees[0].base_ref, "HEAD");
        assert_eq!(worktrees[0].merge_target_ref, "main");
    }

    #[test]
    fn remove_worktree_cleans_up() {
        let tmp = init_test_repo();
        let root = tmp.path();

        let wt = create_worktree(root, None, "task-remove", "HEAD", "main").unwrap();
        assert!(wt.path.exists());

        remove_worktree(root, "task-remove").unwrap();

        assert!(!wt.path.exists(), "worktree directory should be removed");

        // Branch should be deleted
        let branches = git(root, &["branch", "--list", "kernel/task-remove"]).unwrap();
        assert!(branches.trim().is_empty(), "branch should be deleted");

        // Metadata should be removed
        let meta_file = meta_dir(root).join("task-remove.json");
        assert!(!meta_file.exists(), "metadata file should be removed");
    }

    #[test]
    fn remove_worktree_with_changes_fails() {
        let tmp = init_test_repo();
        let root = tmp.path();

        let wt = create_worktree(root, None, "task-dirty", "HEAD", "main").unwrap();

        // Make a change in the worktree
        std::fs::write(wt.path.join("new-file.txt"), "hello").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(&wt.path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "change"])
            .current_dir(&wt.path)
            .output()
            .unwrap();

        // remove_worktree uses -d which will fail for unmerged branches
        let result = remove_worktree(root, "task-dirty");
        // git worktree remove should succeed (clean working tree),
        // but git branch -d will fail because the branch has unmerged commits
        assert!(result.is_err());
    }

    #[test]
    fn worktree_for_task_returns_none() {
        let tmp = init_test_repo();
        let result = worktree_for_task(tmp.path(), "any-task").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn branch_naming_follows_convention() {
        let tmp = init_test_repo();
        let root = tmp.path();

        let wt = create_worktree(root, None, "task-auth-middleware", "HEAD", "main").unwrap();
        assert_eq!(wt.branch, "kernel/task-auth-middleware");
        assert!(wt.path.ends_with(".kernel/worktrees/task-auth-middleware"));
    }

    #[test]
    fn slug_from_branch_strips_prefix() {
        assert_eq!(slug_from_branch("kernel/task-foo"), "task-foo");
        assert_eq!(slug_from_branch("other/branch"), "other/branch");
        assert_eq!(slug_from_branch("plain"), "plain");
    }

    #[test]
    fn worktree_serde_roundtrip() {
        let wt = Worktree {
            path: PathBuf::from("/tmp/worktree"),
            branch: "kernel/task-test".to_string(),
            task_id: Some("t-1".to_string()),
            base_ref: "main".to_string(),
            base_commit: "abc123".to_string(),
            merge_target_ref: "main".to_string(),
            created_at: "2026-01-01T00:00:00+00:00".to_string(),
        };

        let json = serde_json::to_string(&wt).unwrap();
        let parsed: Worktree = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.branch, wt.branch);
        assert_eq!(parsed.task_id, wt.task_id);
        assert_eq!(parsed.base_commit, wt.base_commit);
    }
}
