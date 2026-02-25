use std::path::Path;
use tracing::{debug, error, info};

use super::worktree::{list_worktrees, WorktreeError};

fn git(project_root: &Path, args: &[&str]) -> Result<String, WorktreeError> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(WorktreeError::Git(stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Fully cleans up a merged worktree: removes the git worktree, deletes the
/// branch, and removes leftover directories and metadata.
///
/// Best-effort: continues through individual step failures and returns the
/// first error encountered (if any) after all cleanup steps have been attempted.
pub fn cleanup_merged_worktree(
    project_root: &Path,
    branch_name: &str,
) -> Result<(), WorktreeError> {
    info!(branch = %branch_name, "cleaning up merged worktree");
    let full_branch = format!("kernel/{branch_name}");
    let worktree_path = project_root
        .join(".kernel")
        .join("worktrees")
        .join(branch_name);
    let meta_file = project_root
        .join(".kernel")
        .join("worktree-meta")
        .join(format!("{branch_name}.json"));

    // 1. Remove git worktree (non-fatal if already gone)
    let worktree_path_str = worktree_path.to_string_lossy().to_string();
    debug!(path = %worktree_path_str, "removing git worktree");
    let _ = git(project_root, &["worktree", "remove", &worktree_path_str]);

    // 2. Prune stale worktree bookkeeping so branch -d doesn't see a lock
    debug!("pruning stale worktree bookkeeping");
    let _ = git(project_root, &["worktree", "prune"]);

    // 3. Delete branch — this is the real gate: fails if branch has unmerged commits
    debug!(branch = %full_branch, "deleting branch");
    git(project_root, &["branch", "-d", &full_branch]).map_err(|e| {
        error!(branch = %full_branch, error = %e, "failed to delete branch (unmerged commits?)");
        e
    })?;

    // 4. Remove worktree directory if it still exists (belt-and-suspenders)
    if worktree_path.exists() {
        debug!(path = %worktree_path.display(), "removing leftover worktree directory");
        std::fs::remove_dir_all(&worktree_path)?;
    }

    // 5. Remove metadata file
    if meta_file.exists() {
        debug!(path = %meta_file.display(), "removing metadata file");
        std::fs::remove_file(&meta_file)?;
    }

    info!(branch = %branch_name, "cleanup complete");
    Ok(())
}

/// Garbage-collects worktrees that have no matching active task.
///
/// Lists all kernel-managed worktrees and removes any whose `task_id` is not
/// in `active_task_ids`. Worktrees without a task_id are also removed.
///
/// Returns the list of branch names that were cleaned up.
pub fn garbage_collect_worktrees(
    project_root: &Path,
    active_task_ids: &[String],
) -> Result<Vec<String>, WorktreeError> {
    info!(active_tasks = active_task_ids.len(), "starting worktree garbage collection");
    let worktrees = list_worktrees(project_root)?;
    let mut removed = Vec::new();

    for wt in worktrees {
        let is_active = wt
            .task_id
            .as_ref()
            .is_some_and(|id| active_task_ids.contains(id));

        if !is_active {
            let slug = wt.branch.strip_prefix("kernel/").unwrap_or(&wt.branch);
            debug!(branch = %wt.branch, task_id = ?wt.task_id, "collecting inactive worktree");

            // Best-effort: log the branch as removed even if cleanup partially fails,
            // since the worktree is abandoned and should not block other cleanup.
            removed.push(wt.branch.clone());

            let _ = cleanup_merged_worktree(project_root, slug);
        }
    }

    info!(removed_count = removed.len(), "garbage collection complete");
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::worktree::create_worktree;
    use std::process::Command;

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
    fn cleanup_merged_worktree_removes_everything() {
        let tmp = init_test_repo();
        let root = tmp.path();

        let wt = create_worktree(root, Some("t-1"), "task-clean", "HEAD", "main").unwrap();
        assert!(wt.path.exists());

        // Branch is at same commit as main (no new commits), so -d will succeed
        cleanup_merged_worktree(root, "task-clean").unwrap();

        // Worktree directory gone
        assert!(!wt.path.exists());

        // Branch gone
        let branches = git(root, &["branch", "--list", "kernel/task-clean"]).unwrap();
        assert!(branches.trim().is_empty());

        // Metadata gone
        let meta = root
            .join(".kernel")
            .join("worktree-meta")
            .join("task-clean.json");
        assert!(!meta.exists());
    }

    #[test]
    fn cleanup_unmerged_worktree_returns_error() {
        let tmp = init_test_repo();
        let root = tmp.path();

        let wt = create_worktree(root, None, "task-unmerged", "HEAD", "main").unwrap();

        // Add a commit so the branch diverges from main
        std::fs::write(wt.path.join("new.txt"), "content").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(&wt.path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "diverge"])
            .current_dir(&wt.path)
            .output()
            .unwrap();

        let result = cleanup_merged_worktree(root, "task-unmerged");
        // git branch -d will fail because the branch has unmerged commits
        assert!(result.is_err());
    }

    #[test]
    fn cleanup_already_removed_worktree_cleans_leftovers() {
        let tmp = init_test_repo();
        let root = tmp.path();

        create_worktree(root, Some("t-2"), "task-partial", "HEAD", "main").unwrap();

        // Manually remove the git worktree first (simulating partial cleanup)
        let wt_path = root.join(".kernel/worktrees/task-partial");
        let wt_path_str = wt_path.to_string_lossy().to_string();
        git(root, &["worktree", "remove", &wt_path_str]).unwrap();

        // cleanup_merged_worktree should still clean up branch + metadata
        cleanup_merged_worktree(root, "task-partial").unwrap();

        let branches = git(root, &["branch", "--list", "kernel/task-partial"]).unwrap();
        assert!(branches.trim().is_empty());

        let meta = root
            .join(".kernel")
            .join("worktree-meta")
            .join("task-partial.json");
        assert!(!meta.exists());
    }

    #[test]
    fn gc_removes_worktrees_without_active_tasks() {
        let tmp = init_test_repo();
        let root = tmp.path();

        create_worktree(root, Some("active-1"), "task-keep", "HEAD", "main").unwrap();
        create_worktree(root, Some("stale-1"), "task-stale", "HEAD", "main").unwrap();
        create_worktree(root, None, "task-orphan", "HEAD", "main").unwrap();

        let active = vec!["active-1".to_string()];
        let removed = garbage_collect_worktrees(root, &active).unwrap();

        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&"kernel/task-stale".to_string()));
        assert!(removed.contains(&"kernel/task-orphan".to_string()));

        // Active worktree should still exist
        let remaining = list_worktrees(root).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].branch, "kernel/task-keep");
    }

    #[test]
    fn gc_with_all_active_removes_nothing() {
        let tmp = init_test_repo();
        let root = tmp.path();

        create_worktree(root, Some("a"), "task-a", "HEAD", "main").unwrap();
        create_worktree(root, Some("b"), "task-b", "HEAD", "main").unwrap();

        let active = vec!["a".to_string(), "b".to_string()];
        let removed = garbage_collect_worktrees(root, &active).unwrap();

        assert!(removed.is_empty());
        assert_eq!(list_worktrees(root).unwrap().len(), 2);
    }

    #[test]
    fn gc_with_no_worktrees_returns_empty() {
        let tmp = init_test_repo();
        let removed = garbage_collect_worktrees(tmp.path(), &[]).unwrap();
        assert!(removed.is_empty());
    }

    #[test]
    fn gc_with_empty_active_list_removes_all() {
        let tmp = init_test_repo();
        let root = tmp.path();

        create_worktree(root, Some("x"), "task-x", "HEAD", "main").unwrap();
        create_worktree(root, Some("y"), "task-y", "HEAD", "main").unwrap();

        let removed = garbage_collect_worktrees(root, &[]).unwrap();

        assert_eq!(removed.len(), 2);
        assert!(list_worktrees(root).unwrap().is_empty());
    }
}
