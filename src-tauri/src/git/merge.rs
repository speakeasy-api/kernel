use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::diff::{parse_unified_diff, FileDiff, Hunk};

const KERNEL_DIR: &str = ".kernel";
const WORKTREES_DIR: &str = "worktrees";
const BRANCH_PREFIX: &str = "kernel/";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MergeStrategy {
    Squash,
    Merge,
    CherryPick,
}

impl Default for MergeStrategy {
    fn default() -> Self {
        Self::Squash
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialMergeResult {
    pub merged_commit: String,
    pub rejected_hunks: Vec<(String, Vec<usize>)>,
}

#[derive(Debug)]
pub enum MergeError {
    Git(String),
    Io(std::io::Error),
    Conflict(Vec<String>),
    Parse(String),
}

impl fmt::Display for MergeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(msg) => write!(f, "git error: {msg}"),
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Conflict(files) => write!(f, "merge conflict in: {}", files.join(", ")),
            Self::Parse(msg) => write!(f, "parse error: {msg}"),
        }
    }
}

impl std::error::Error for MergeError {}

impl From<std::io::Error> for MergeError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

fn worktree_path(project_root: &Path, branch: &str) -> PathBuf {
    let slug = branch.strip_prefix(BRANCH_PREFIX).unwrap_or(branch);
    project_root.join(KERNEL_DIR).join(WORKTREES_DIR).join(slug)
}

fn git(dir: &Path, args: &[&str]) -> Result<String, MergeError> {
    let output = Command::new("git").args(args).current_dir(dir).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(MergeError::Git(stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Tries a git command and returns Ok(stdout) on success, or the stderr string
/// on failure (without converting to MergeError). Used when we need to inspect
/// the error to distinguish conflicts from other failures.
fn git_try(dir: &Path, args: &[&str]) -> Result<String, (String, i32)> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| (e.to_string(), -1))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let code = output.status.code().unwrap_or(-1);
        return Err((stderr, code));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Detects merge conflicts by listing unmerged files.
fn detect_conflicts(dir: &Path) -> Result<Vec<String>, MergeError> {
    let output = git(dir, &["diff", "--name-only", "--diff-filter=U"])?;
    Ok(output
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

/// Merges `branch` into `merge_target_ref` using the specified strategy.
///
/// - **Squash**: switches to `merge_target_ref`, squash-merges the branch,
///   and creates a single commit. Returns the new commit SHA.
/// - **Merge**: switches to `merge_target_ref` and performs a regular merge,
///   preserving branch history. Returns the merge commit SHA.
/// - **CherryPick**: switches to `merge_target_ref` and cherry-picks all
///   commits from the branch (relative to `base_commit`). Returns the SHA
///   of the last cherry-picked commit.
///
/// Merge conflicts are detected and returned as `MergeError::Conflict`.
pub fn merge_to_target(
    project_root: &Path,
    branch: &str,
    base_commit: &str,
    merge_target_ref: &str,
    strategy: MergeStrategy,
) -> Result<String, MergeError> {
    // Work from the project root (not a worktree) so we can checkout the target
    git(project_root, &["checkout", merge_target_ref])?;

    let result = match strategy {
        MergeStrategy::Squash => do_squash_merge(project_root, branch),
        MergeStrategy::Merge => do_full_merge(project_root, branch),
        MergeStrategy::CherryPick => do_cherry_pick(project_root, branch, base_commit),
    };

    // On conflict, abort the in-progress merge/cherry-pick before returning.
    // Squash merges don't set MERGE_HEAD, so --abort won't work; reset --merge
    // handles all cases.
    if let Err(MergeError::Conflict(_)) = &result {
        let _ = git_try(project_root, &["merge", "--abort"]);
        let _ = git_try(project_root, &["cherry-pick", "--abort"]);
        let _ = git_try(project_root, &["reset", "--merge"]);
    }

    result
}

fn do_squash_merge(dir: &Path, branch: &str) -> Result<String, MergeError> {
    match git_try(dir, &["merge", "--squash", branch]) {
        Ok(_) => {}
        Err((stderr, _)) => {
            let conflicts = detect_conflicts(dir)?;
            if !conflicts.is_empty() {
                return Err(MergeError::Conflict(conflicts));
            }
            return Err(MergeError::Git(stderr));
        }
    }

    let msg = format!("Squash merge branch '{branch}'");
    git(dir, &["commit", "-m", &msg])?;

    let sha = git(dir, &["rev-parse", "HEAD"])?.trim().to_string();
    Ok(sha)
}

fn do_full_merge(dir: &Path, branch: &str) -> Result<String, MergeError> {
    let msg = format!("Merge branch '{branch}'");
    match git_try(dir, &["merge", "--no-ff", branch, "-m", &msg]) {
        Ok(_) => {}
        Err((stderr, _)) => {
            let conflicts = detect_conflicts(dir)?;
            if !conflicts.is_empty() {
                return Err(MergeError::Conflict(conflicts));
            }
            return Err(MergeError::Git(stderr));
        }
    }

    let sha = git(dir, &["rev-parse", "HEAD"])?.trim().to_string();
    Ok(sha)
}

fn do_cherry_pick(dir: &Path, branch: &str, base_commit: &str) -> Result<String, MergeError> {
    // Get the list of commits to cherry-pick (oldest first)
    let log_output = git(
        dir,
        &["rev-list", "--reverse", &format!("{base_commit}..{branch}")],
    )?;
    let commits: Vec<&str> = log_output.lines().filter(|l| !l.is_empty()).collect();

    if commits.is_empty() {
        return Err(MergeError::Git("no commits to cherry-pick".into()));
    }

    for commit in &commits {
        match git_try(dir, &["cherry-pick", commit]) {
            Ok(_) => {}
            Err((stderr, _)) => {
                let conflicts = detect_conflicts(dir)?;
                if !conflicts.is_empty() {
                    return Err(MergeError::Conflict(conflicts));
                }
                return Err(MergeError::Git(stderr));
            }
        }
    }

    let sha = git(dir, &["rev-parse", "HEAD"])?.trim().to_string();
    Ok(sha)
}

/// Applies only selected hunks from a branch's diff to `merge_target_ref`.
///
/// `accepted_hunks` maps file paths to the (0-based) hunk indexes to accept.
/// The function generates the full diff, filters to accepted hunks, applies
/// them via `git apply`, and commits. Hunks not in `accepted_hunks` are
/// returned as `rejected_hunks` in the result.
pub fn partial_merge(
    project_root: &Path,
    branch: &str,
    base_commit: &str,
    merge_target_ref: &str,
    accepted_hunks: &[(String, Vec<usize>)],
) -> Result<PartialMergeResult, MergeError> {
    // Get the full diff from the branch
    let wt_path = worktree_path(project_root, branch);
    let range = format!("{base_commit}..HEAD");
    let full_diff =
        git(&wt_path, &["diff", "-M", &range]).map_err(|e| MergeError::Git(format!("{e}")))?;

    let file_diffs =
        parse_unified_diff(&full_diff).map_err(|e| MergeError::Parse(format!("{e}")))?;

    // Build a lookup of accepted hunk indexes per file
    let accepted_map: std::collections::HashMap<&str, &[usize]> = accepted_hunks
        .iter()
        .map(|(path, idxs)| (path.as_str(), idxs.as_slice()))
        .collect();

    // Build accepted patch and track rejected hunks
    let mut patch_lines: Vec<String> = Vec::new();
    let mut rejected: Vec<(String, Vec<usize>)> = Vec::new();

    for file_diff in &file_diffs {
        let accepted_idxs = accepted_map.get(file_diff.path.as_str());

        let mut file_accepted: Vec<usize> = Vec::new();
        let mut file_rejected: Vec<usize> = Vec::new();

        for (idx, _hunk) in file_diff.hunks.iter().enumerate() {
            if accepted_idxs.map_or(false, |idxs| idxs.contains(&idx)) {
                file_accepted.push(idx);
            } else {
                file_rejected.push(idx);
            }
        }

        if !file_accepted.is_empty() {
            emit_file_patch(&mut patch_lines, file_diff, &file_accepted);
        }

        if !file_rejected.is_empty() {
            rejected.push((file_diff.path.clone(), file_rejected));
        }
    }

    if patch_lines.is_empty() {
        return Err(MergeError::Git("no hunks selected for merge".into()));
    }

    // Switch to merge target and apply the patch
    git(project_root, &["checkout", merge_target_ref])?;

    let patch_content = patch_lines.join("\n") + "\n";
    apply_patch(project_root, &patch_content)?;

    git(project_root, &["add", "-A"])?;

    let msg = format!("Partial merge from branch '{branch}'");
    git(project_root, &["commit", "-m", &msg])?;

    let sha = git(project_root, &["rev-parse", "HEAD"])?
        .trim()
        .to_string();

    Ok(PartialMergeResult {
        merged_commit: sha,
        rejected_hunks: rejected,
    })
}

/// Reconstructs a unified diff patch for a file, including only the specified hunks.
fn emit_file_patch(out: &mut Vec<String>, file_diff: &FileDiff, accepted_idxs: &[usize]) {
    use super::diff::FileStatus;

    // Emit diff header
    let a_path = match &file_diff.status {
        FileStatus::Added => "/dev/null".to_string(),
        FileStatus::Renamed { from } => format!("a/{from}"),
        _ => format!("a/{}", file_diff.path),
    };
    let b_path = match &file_diff.status {
        FileStatus::Deleted => "/dev/null".to_string(),
        _ => format!("b/{}", file_diff.path),
    };

    out.push(format!("diff --git {a_path} {b_path}"));

    match &file_diff.status {
        FileStatus::Added => out.push("new file mode 100644".to_string()),
        FileStatus::Deleted => out.push("deleted file mode 100644".to_string()),
        _ => {}
    }

    out.push(format!("--- {a_path}"));
    out.push(format!("+++ {b_path}"));

    for &idx in accepted_idxs {
        if let Some(hunk) = file_diff.hunks.get(idx) {
            emit_hunk(out, hunk);
        }
    }
}

fn emit_hunk(out: &mut Vec<String>, hunk: &Hunk) {
    use super::diff::LineKind;

    out.push(hunk.header.clone());
    for line in &hunk.lines {
        let prefix = match line.kind {
            LineKind::Context => " ",
            LineKind::Add => "+",
            LineKind::Remove => "-",
        };
        out.push(format!("{prefix}{}", line.content));
    }
}

fn apply_patch(dir: &Path, patch: &str) -> Result<(), MergeError> {
    let mut child = Command::new("git")
        .args(["apply", "--index", "-"])
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    use std::io::Write;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(patch.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(MergeError::Git(format!("git apply failed: {stderr}")));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as Cmd;

    fn init_test_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();

        Cmd::new("git")
            .args(["init"])
            .current_dir(p)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(p)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(p)
            .output()
            .unwrap();

        std::fs::write(p.join("README.md"), "# test\n").unwrap();
        Cmd::new("git")
            .args(["add", "."])
            .current_dir(p)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(p)
            .output()
            .unwrap();

        tmp
    }

    fn setup_branch_with_changes(root: &Path, branch_slug: &str) -> (String, String) {
        let base_commit = git(root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let full_branch = format!("{BRANCH_PREFIX}{branch_slug}");
        let wt_path = worktree_path(root, &full_branch);

        std::fs::create_dir_all(root.join(KERNEL_DIR).join(WORKTREES_DIR)).unwrap();
        let wt_str = wt_path.to_string_lossy().to_string();
        git(
            root,
            &["worktree", "add", &wt_str, "-b", &full_branch, "HEAD"],
        )
        .unwrap();

        // Make changes in the worktree
        std::fs::write(wt_path.join("feature.txt"), "new feature\n").unwrap();
        Cmd::new("git")
            .args(["add", "."])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "add feature"])
            .current_dir(&wt_path)
            .output()
            .unwrap();

        (base_commit, full_branch)
    }

    // ── MergeStrategy defaults ──────────────────────────────────────────

    #[test]
    fn merge_strategy_default_is_squash() {
        assert_eq!(MergeStrategy::default(), MergeStrategy::Squash);
    }

    // ── Squash merge ────────────────────────────────────────────────────

    #[test]
    fn squash_merge_produces_single_commit() {
        let tmp = init_test_repo();
        let root = tmp.path();
        let (base_commit, branch) = setup_branch_with_changes(root, "task-squash");

        let head_before = git(root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();

        let sha =
            merge_to_target(root, &branch, &base_commit, "main", MergeStrategy::Squash).unwrap();

        assert!(!sha.is_empty());
        assert_ne!(sha, head_before);

        // Verify it's a single commit (not a merge commit — one parent)
        let parents = git(root, &["rev-list", "--parents", "-n1", &sha])
            .unwrap()
            .trim()
            .to_string();
        let parent_count = parents.split_whitespace().count() - 1; // minus the commit itself
        assert_eq!(
            parent_count, 1,
            "squash merge should produce a non-merge commit"
        );

        // Verify the file exists on main
        assert!(root.join("feature.txt").exists());
    }

    #[test]
    fn squash_merge_with_multiple_commits() {
        let tmp = init_test_repo();
        let root = tmp.path();
        let base_commit = git(root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let branch = format!("{BRANCH_PREFIX}task-multi-squash");
        let wt_path = worktree_path(root, &branch);

        std::fs::create_dir_all(root.join(KERNEL_DIR).join(WORKTREES_DIR)).unwrap();
        let wt_str = wt_path.to_string_lossy().to_string();
        git(root, &["worktree", "add", &wt_str, "-b", &branch, "HEAD"]).unwrap();

        // Make two commits
        std::fs::write(wt_path.join("a.txt"), "a\n").unwrap();
        Cmd::new("git")
            .args(["add", "."])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "first"])
            .current_dir(&wt_path)
            .output()
            .unwrap();

        std::fs::write(wt_path.join("b.txt"), "b\n").unwrap();
        Cmd::new("git")
            .args(["add", "."])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "second"])
            .current_dir(&wt_path)
            .output()
            .unwrap();

        let sha =
            merge_to_target(root, &branch, &base_commit, "main", MergeStrategy::Squash).unwrap();

        // Should still be one commit on main
        let log = git(
            root,
            &["rev-list", "--count", &format!("{base_commit}..{sha}")],
        )
        .unwrap()
        .trim()
        .to_string();
        assert_eq!(log, "1", "squash should collapse multiple commits into one");

        // Both files should exist
        assert!(root.join("a.txt").exists());
        assert!(root.join("b.txt").exists());
    }

    // ── Full merge ──────────────────────────────────────────────────────

    #[test]
    fn full_merge_preserves_history() {
        let tmp = init_test_repo();
        let root = tmp.path();
        let (base_commit, branch) = setup_branch_with_changes(root, "task-full");

        let sha =
            merge_to_target(root, &branch, &base_commit, "main", MergeStrategy::Merge).unwrap();

        assert!(!sha.is_empty());

        // Verify it's a merge commit (two parents)
        let parents = git(root, &["rev-list", "--parents", "-n1", &sha])
            .unwrap()
            .trim()
            .to_string();
        let parent_count = parents.split_whitespace().count() - 1;
        assert_eq!(parent_count, 2, "full merge should produce a merge commit");

        assert!(root.join("feature.txt").exists());
    }

    // ── Cherry-pick ─────────────────────────────────────────────────────

    #[test]
    fn cherry_pick_applies_commits() {
        let tmp = init_test_repo();
        let root = tmp.path();
        let (base_commit, branch) = setup_branch_with_changes(root, "task-cp");

        let sha = merge_to_target(
            root,
            &branch,
            &base_commit,
            "main",
            MergeStrategy::CherryPick,
        )
        .unwrap();

        assert!(!sha.is_empty());
        assert!(root.join("feature.txt").exists());
    }

    // ── Conflict detection ──────────────────────────────────────────────

    #[test]
    fn squash_merge_detects_conflict() {
        let tmp = init_test_repo();
        let root = tmp.path();
        let (base_commit, branch) = setup_branch_with_changes(root, "task-conflict");

        // Create a conflicting change on main
        std::fs::write(root.join("feature.txt"), "conflicting content\n").unwrap();
        Cmd::new("git")
            .args(["add", "."])
            .current_dir(root)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "conflict on main"])
            .current_dir(root)
            .output()
            .unwrap();

        let result = merge_to_target(root, &branch, &base_commit, "main", MergeStrategy::Squash);

        assert!(result.is_err());
        match result.unwrap_err() {
            MergeError::Conflict(files) => {
                assert!(files.contains(&"feature.txt".to_string()));
            }
            other => panic!("expected Conflict, got: {other}"),
        }

        // Verify the merge was aborted and we're clean
        let status = git(root, &["status", "--porcelain"]).unwrap();
        assert!(
            status.trim().is_empty(),
            "working tree should be clean after abort"
        );
    }

    // ── Partial merge ───────────────────────────────────────────────────

    #[test]
    fn partial_merge_applies_selected_hunks() {
        let tmp = init_test_repo();
        let root = tmp.path();

        // Set up a file with content that will produce multiple hunks
        std::fs::write(
            root.join("code.txt"),
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n\
             line11\nline12\nline13\nline14\nline15\nline16\nline17\nline18\nline19\nline20\n",
        )
        .unwrap();
        Cmd::new("git")
            .args(["add", "."])
            .current_dir(root)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "add code.txt"])
            .current_dir(root)
            .output()
            .unwrap();

        let base_commit = git(root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let branch = format!("{BRANCH_PREFIX}task-partial");
        let wt_path = worktree_path(root, &branch);

        std::fs::create_dir_all(root.join(KERNEL_DIR).join(WORKTREES_DIR)).unwrap();
        let wt_str = wt_path.to_string_lossy().to_string();
        git(root, &["worktree", "add", &wt_str, "-b", &branch, "HEAD"]).unwrap();

        // Modify both ends of the file to create two hunks
        std::fs::write(
            wt_path.join("code.txt"),
            "CHANGED1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n\
             line11\nline12\nline13\nline14\nline15\nline16\nline17\nline18\nline19\nCHANGED20\n",
        )
        .unwrap();
        Cmd::new("git")
            .args(["add", "."])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "modify both ends"])
            .current_dir(&wt_path)
            .output()
            .unwrap();

        // Accept only the first hunk (index 0), reject hunk 1
        let accepted = vec![("code.txt".to_string(), vec![0])];
        let result = partial_merge(root, &branch, &base_commit, "main", &accepted).unwrap();

        assert!(!result.merged_commit.is_empty());

        // Rejected hunks should contain hunk index 1 for code.txt
        assert_eq!(result.rejected_hunks.len(), 1);
        assert_eq!(result.rejected_hunks[0].0, "code.txt");
        assert!(result.rejected_hunks[0].1.contains(&1));

        // Verify the first change was applied
        let content = std::fs::read_to_string(root.join("code.txt")).unwrap();
        assert!(content.starts_with("CHANGED1\n"));
    }

    #[test]
    fn partial_merge_returns_all_rejected_when_file_not_in_accepted() {
        let tmp = init_test_repo();
        let root = tmp.path();

        let base_commit = git(root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let branch = format!("{BRANCH_PREFIX}task-partial-reject");
        let wt_path = worktree_path(root, &branch);

        std::fs::create_dir_all(root.join(KERNEL_DIR).join(WORKTREES_DIR)).unwrap();
        let wt_str = wt_path.to_string_lossy().to_string();
        git(root, &["worktree", "add", &wt_str, "-b", &branch, "HEAD"]).unwrap();

        // Add a new file (will have 1 hunk)
        std::fs::write(wt_path.join("new.txt"), "new content\n").unwrap();
        // Modify existing file
        std::fs::write(wt_path.join("README.md"), "# changed\n").unwrap();
        Cmd::new("git")
            .args(["add", "."])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "two files changed"])
            .current_dir(&wt_path)
            .output()
            .unwrap();

        // Accept only README.md changes, not new.txt
        let accepted = vec![("README.md".to_string(), vec![0])];
        let result = partial_merge(root, &branch, &base_commit, "main", &accepted).unwrap();

        // new.txt should appear in rejected
        let rejected_files: Vec<&str> = result
            .rejected_hunks
            .iter()
            .map(|(f, _)| f.as_str())
            .collect();
        assert!(rejected_files.contains(&"new.txt"));
    }

    #[test]
    fn partial_merge_errors_on_empty_selection() {
        let tmp = init_test_repo();
        let root = tmp.path();

        let base_commit = git(root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let branch = format!("{BRANCH_PREFIX}task-partial-empty");
        let wt_path = worktree_path(root, &branch);

        std::fs::create_dir_all(root.join(KERNEL_DIR).join(WORKTREES_DIR)).unwrap();
        let wt_str = wt_path.to_string_lossy().to_string();
        git(root, &["worktree", "add", &wt_str, "-b", &branch, "HEAD"]).unwrap();

        std::fs::write(wt_path.join("file.txt"), "content\n").unwrap();
        Cmd::new("git")
            .args(["add", "."])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "change"])
            .current_dir(&wt_path)
            .output()
            .unwrap();

        // Pass empty accepted hunks
        let accepted: Vec<(String, Vec<usize>)> = vec![];
        let result = partial_merge(root, &branch, &base_commit, "main", &accepted);
        assert!(result.is_err());
    }

    // ── Serde roundtrip ─────────────────────────────────────────────────

    #[test]
    fn merge_strategy_serde_roundtrip() {
        for strategy in [
            MergeStrategy::Squash,
            MergeStrategy::Merge,
            MergeStrategy::CherryPick,
        ] {
            let json = serde_json::to_string(&strategy).unwrap();
            let parsed: MergeStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, strategy);
        }
    }

    #[test]
    fn partial_merge_result_serde_roundtrip() {
        let result = PartialMergeResult {
            merged_commit: "abc123".to_string(),
            rejected_hunks: vec![("file.rs".to_string(), vec![1, 3])],
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: PartialMergeResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.merged_commit, result.merged_commit);
        assert_eq!(parsed.rejected_hunks, result.rejected_hunks);
    }
}
