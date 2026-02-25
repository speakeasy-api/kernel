use serde::{Deserialize, Serialize};
use std::fmt;
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::Lines;
use tracing::debug;

const KERNEL_DIR: &str = ".kernel";
const WORKTREES_DIR: &str = "worktrees";
const BRANCH_PREFIX: &str = "kernel/";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffStat {
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub status: FileStatus,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed { from: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LineKind {
    Context,
    Add,
    Remove,
}

#[derive(Debug)]
pub enum DiffError {
    Git(String),
    Io(std::io::Error),
    Parse(String),
}

impl fmt::Display for DiffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(msg) => write!(f, "git error: {msg}"),
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Parse(msg) => write!(f, "parse error: {msg}"),
        }
    }
}

impl std::error::Error for DiffError {}

impl From<std::io::Error> for DiffError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

fn worktree_path(project_root: &Path, branch: &str) -> PathBuf {
    let slug = branch.strip_prefix(BRANCH_PREFIX).unwrap_or(branch);
    project_root.join(KERNEL_DIR).join(WORKTREES_DIR).join(slug)
}

fn git(dir: &Path, args: &[&str]) -> Result<String, DiffError> {
    let output = Command::new("git").args(args).current_dir(dir).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(DiffError::Git(stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Generates structured diffs between `base_commit` and HEAD in the worktree
/// associated with `branch`.
pub fn diff_for_task(
    project_root: &Path,
    branch: &str,
    base_commit: &str,
) -> Result<Vec<FileDiff>, DiffError> {
    debug!(branch = %branch, base_commit = %base_commit, "computing diff for task");
    let wt_path = worktree_path(project_root, branch);
    let range = format!("{base_commit}..HEAD");
    let output = git(&wt_path, &["diff", "-M", &range])?;
    let files = parse_unified_diff(&output)?;
    debug!(files_changed = files.len(), "diff computed");
    Ok(files)
}

/// Returns summary diff statistics between `base_commit` and HEAD in the
/// worktree associated with `branch`.
pub fn diff_stat(
    project_root: &Path,
    branch: &str,
    base_commit: &str,
) -> Result<DiffStat, DiffError> {
    debug!(branch = %branch, base_commit = %base_commit, "computing diff stat");
    let wt_path = worktree_path(project_root, branch);
    let range = format!("{base_commit}..HEAD");
    let output = git(&wt_path, &["diff", "--shortstat", &range])?;
    let stat = parse_shortstat(&output)?;
    debug!(
        files_changed = stat.files_changed,
        insertions = stat.insertions,
        deletions = stat.deletions,
        "diff stat computed"
    );
    Ok(stat)
}

// ── Parsing ──────────────────────────────────────────────────────────────

pub(crate) fn parse_unified_diff(input: &str) -> Result<Vec<FileDiff>, DiffError> {
    let mut files = Vec::new();
    let mut lines = input.lines().peekable();

    while lines.peek().is_some() {
        if lines.peek().map_or(false, |l| l.starts_with("diff --git ")) {
            files.push(parse_file_diff(&mut lines)?);
        } else {
            lines.next();
        }
    }

    Ok(files)
}

fn parse_file_diff(lines: &mut Peekable<Lines<'_>>) -> Result<FileDiff, DiffError> {
    let diff_line = lines
        .next()
        .ok_or_else(|| DiffError::Parse("expected diff header".into()))?;
    let mut path = parse_diff_path(diff_line);
    let mut status = FileStatus::Modified;
    let mut rename_from: Option<String> = None;

    // Consume extended header lines (index, mode, similarity, rename)
    while let Some(&line) = lines.peek() {
        if line.starts_with("diff --git ") || line.starts_with("--- ") || line.starts_with("@@ ") {
            break;
        }
        if line.starts_with("new file mode") {
            status = FileStatus::Added;
        } else if line.starts_with("deleted file mode") {
            status = FileStatus::Deleted;
        } else if let Some(from) = line.strip_prefix("rename from ") {
            rename_from = Some(from.to_string());
        } else if line.starts_with("rename to ") && rename_from.is_some() {
            status = FileStatus::Renamed {
                from: rename_from.take().unwrap(),
            };
        }
        lines.next();
    }

    // --- a/old and +++ b/new
    if lines.peek().map_or(false, |l| l.starts_with("--- ")) {
        lines.next();
    }
    if let Some(&plus_line) = lines.peek() {
        if plus_line.starts_with("+++ ") {
            if let Some(p) = plus_line.strip_prefix("+++ b/") {
                path = p.to_string();
            }
            // +++ /dev/null means deleted — keep path from diff header
            lines.next();
        }
    }

    // Parse hunks
    let mut hunks = Vec::new();
    while lines.peek().map_or(false, |l| l.starts_with("@@ ")) {
        hunks.push(parse_hunk(lines));
    }

    Ok(FileDiff {
        path,
        status,
        hunks,
    })
}

fn parse_hunk(lines: &mut Peekable<Lines<'_>>) -> Hunk {
    let header = lines.next().unwrap().to_string();
    let mut diff_lines = Vec::new();

    while let Some(&line) = lines.peek() {
        if line.starts_with("diff --git ") || line.starts_with("@@ ") {
            break;
        }
        // Skip "\ No newline at end of file" markers
        if line.starts_with('\\') {
            lines.next();
            continue;
        }

        let (kind, content) = if let Some(rest) = line.strip_prefix('+') {
            (LineKind::Add, rest.to_string())
        } else if let Some(rest) = line.strip_prefix('-') {
            (LineKind::Remove, rest.to_string())
        } else if let Some(rest) = line.strip_prefix(' ') {
            (LineKind::Context, rest.to_string())
        } else {
            // Empty context line (no leading space when line itself is empty)
            (LineKind::Context, line.to_string())
        };

        diff_lines.push(DiffLine { kind, content });
        lines.next();
    }

    Hunk {
        header,
        lines: diff_lines,
    }
}

fn parse_diff_path(diff_line: &str) -> String {
    let stripped = diff_line.strip_prefix("diff --git ").unwrap_or(diff_line);
    // Format: a/<path> b/<path> — find the last " b/" and take after it
    stripped
        .rfind(" b/")
        .map(|pos| stripped[pos + 3..].to_string())
        .unwrap_or_else(|| stripped.to_string())
}

fn parse_shortstat(input: &str) -> Result<DiffStat, DiffError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(DiffStat {
            files_changed: 0,
            insertions: 0,
            deletions: 0,
        });
    }

    let mut files_changed = 0;
    let mut insertions = 0;
    let mut deletions = 0;

    for part in trimmed.split(',') {
        let part = part.trim();
        if part.contains("file") {
            files_changed = extract_leading_number(part);
        } else if part.contains("insertion") {
            insertions = extract_leading_number(part);
        } else if part.contains("deletion") {
            deletions = extract_leading_number(part);
        }
    }

    Ok(DiffStat {
        files_changed,
        insertions,
        deletions,
    })
}

fn extract_leading_number(s: &str) -> usize {
    s.split_whitespace()
        .find_map(|w| w.parse::<usize>().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as Cmd;

    // ── Unit tests: parser ───────────────────────────────────────────────

    #[test]
    fn parse_diff_path_extracts_path() {
        assert_eq!(
            parse_diff_path("diff --git a/src/main.rs b/src/main.rs"),
            "src/main.rs"
        );
    }

    #[test]
    fn parse_diff_path_handles_nested() {
        assert_eq!(
            parse_diff_path("diff --git a/a/b/c.txt b/a/b/c.txt"),
            "a/b/c.txt"
        );
    }

    #[test]
    fn parse_shortstat_empty() {
        let stat = parse_shortstat("").unwrap();
        assert_eq!(stat.files_changed, 0);
        assert_eq!(stat.insertions, 0);
        assert_eq!(stat.deletions, 0);
    }

    #[test]
    fn parse_shortstat_full() {
        let stat = parse_shortstat(" 3 files changed, 10 insertions(+), 2 deletions(-)").unwrap();
        assert_eq!(stat.files_changed, 3);
        assert_eq!(stat.insertions, 10);
        assert_eq!(stat.deletions, 2);
    }

    #[test]
    fn parse_shortstat_insertions_only() {
        let stat = parse_shortstat(" 1 file changed, 5 insertions(+)").unwrap();
        assert_eq!(stat.files_changed, 1);
        assert_eq!(stat.insertions, 5);
        assert_eq!(stat.deletions, 0);
    }

    #[test]
    fn parse_shortstat_deletions_only() {
        let stat = parse_shortstat(" 2 files changed, 8 deletions(-)").unwrap();
        assert_eq!(stat.files_changed, 2);
        assert_eq!(stat.insertions, 0);
        assert_eq!(stat.deletions, 8);
    }

    #[test]
    fn parse_added_file() {
        let diff = "\
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..ce01362
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+hello
+world";
        let files = parse_unified_diff(diff).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "new.txt");
        assert_eq!(files[0].status, FileStatus::Added);
        assert_eq!(files[0].hunks.len(), 1);
        assert_eq!(files[0].hunks[0].lines.len(), 2);
        assert_eq!(files[0].hunks[0].lines[0].kind, LineKind::Add);
        assert_eq!(files[0].hunks[0].lines[0].content, "hello");
    }

    #[test]
    fn parse_deleted_file() {
        let diff = "\
diff --git a/old.txt b/old.txt
deleted file mode 100644
index ce01362..0000000
--- a/old.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-goodbye
-world";
        let files = parse_unified_diff(diff).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "old.txt");
        assert_eq!(files[0].status, FileStatus::Deleted);
        assert_eq!(files[0].hunks[0].lines.len(), 2);
        assert_eq!(files[0].hunks[0].lines[0].kind, LineKind::Remove);
    }

    #[test]
    fn parse_modified_file() {
        let diff = "\
diff --git a/file.txt b/file.txt
index abc1234..def5678 100644
--- a/file.txt
+++ b/file.txt
@@ -1,3 +1,4 @@
 line1
-old line
+new line
+extra line
 line3";
        let files = parse_unified_diff(diff).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "file.txt");
        assert_eq!(files[0].status, FileStatus::Modified);
        assert_eq!(files[0].hunks.len(), 1);

        let lines = &files[0].hunks[0].lines;
        assert_eq!(
            lines[0],
            DiffLine {
                kind: LineKind::Context,
                content: "line1".into()
            }
        );
        assert_eq!(
            lines[1],
            DiffLine {
                kind: LineKind::Remove,
                content: "old line".into()
            }
        );
        assert_eq!(
            lines[2],
            DiffLine {
                kind: LineKind::Add,
                content: "new line".into()
            }
        );
        assert_eq!(
            lines[3],
            DiffLine {
                kind: LineKind::Add,
                content: "extra line".into()
            }
        );
        assert_eq!(
            lines[4],
            DiffLine {
                kind: LineKind::Context,
                content: "line3".into()
            }
        );
    }

    #[test]
    fn parse_renamed_file() {
        let diff = "\
diff --git a/old_name.txt b/new_name.txt
similarity index 100%
rename from old_name.txt
rename to new_name.txt";
        let files = parse_unified_diff(diff).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "new_name.txt");
        assert_eq!(
            files[0].status,
            FileStatus::Renamed {
                from: "old_name.txt".into()
            }
        );
        assert!(files[0].hunks.is_empty());
    }

    #[test]
    fn parse_multiple_files() {
        let diff = "\
diff --git a/a.txt b/a.txt
new file mode 100644
index 0000000..1234567
--- /dev/null
+++ b/a.txt
@@ -0,0 +1 @@
+aaa
diff --git a/b.txt b/b.txt
index 1234567..abcdefg 100644
--- a/b.txt
+++ b/b.txt
@@ -1 +1 @@
-old
+new";
        let files = parse_unified_diff(diff).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.txt");
        assert_eq!(files[0].status, FileStatus::Added);
        assert_eq!(files[1].path, "b.txt");
        assert_eq!(files[1].status, FileStatus::Modified);
    }

    #[test]
    fn parse_multiple_hunks() {
        let diff = "\
diff --git a/file.txt b/file.txt
index abc..def 100644
--- a/file.txt
+++ b/file.txt
@@ -1,3 +1,3 @@
 a
-b
+B
 c
@@ -10,3 +10,3 @@
 x
-y
+Y
 z";
        let files = parse_unified_diff(diff).unwrap();
        assert_eq!(files[0].hunks.len(), 2);
        assert_eq!(files[0].hunks[0].lines.len(), 4);
        assert_eq!(files[0].hunks[1].lines.len(), 4);
    }

    #[test]
    fn parse_no_newline_marker_skipped() {
        let diff = "\
diff --git a/file.txt b/file.txt
index abc..def 100644
--- a/file.txt
+++ b/file.txt
@@ -1 +1 @@
-old
\\ No newline at end of file
+new
\\ No newline at end of file";
        let files = parse_unified_diff(diff).unwrap();
        let lines = &files[0].hunks[0].lines;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].kind, LineKind::Remove);
        assert_eq!(lines[1].kind, LineKind::Add);
    }

    #[test]
    fn parse_empty_diff() {
        let files = parse_unified_diff("").unwrap();
        assert!(files.is_empty());
    }

    // ── Integration tests ────────────────────────────────────────────────

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

    fn create_worktree(root: &Path, branch: &str) -> (String, PathBuf) {
        let base_commit = git(root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let full_branch = format!("{BRANCH_PREFIX}{branch}");
        let wt_path = root.join(KERNEL_DIR).join(WORKTREES_DIR).join(branch);

        std::fs::create_dir_all(root.join(KERNEL_DIR).join(WORKTREES_DIR)).unwrap();
        let wt_str = wt_path.to_string_lossy().to_string();
        git(
            root,
            &["worktree", "add", &wt_str, "-b", &full_branch, "HEAD"],
        )
        .unwrap();

        (base_commit, wt_path)
    }

    #[test]
    fn diff_for_task_added_file() {
        let tmp = init_test_repo();
        let root = tmp.path();
        let (base_commit, wt_path) = create_worktree(root, "task-add");

        std::fs::write(wt_path.join("new.txt"), "hello\nworld\n").unwrap();
        Cmd::new("git")
            .args(["add", "."])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "add file"])
            .current_dir(&wt_path)
            .output()
            .unwrap();

        let files = diff_for_task(root, "kernel/task-add", &base_commit).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "new.txt");
        assert_eq!(files[0].status, FileStatus::Added);
        assert_eq!(files[0].hunks[0].lines.len(), 2);
    }

    #[test]
    fn diff_for_task_modified_file() {
        let tmp = init_test_repo();
        let root = tmp.path();
        let (base_commit, wt_path) = create_worktree(root, "task-mod");

        std::fs::write(wt_path.join("README.md"), "# modified\n").unwrap();
        Cmd::new("git")
            .args(["add", "."])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "modify"])
            .current_dir(&wt_path)
            .output()
            .unwrap();

        let files = diff_for_task(root, "kernel/task-mod", &base_commit).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "README.md");
        assert_eq!(files[0].status, FileStatus::Modified);
    }

    #[test]
    fn diff_for_task_deleted_file() {
        let tmp = init_test_repo();
        let root = tmp.path();
        let (base_commit, wt_path) = create_worktree(root, "task-del");

        Cmd::new("git")
            .args(["rm", "README.md"])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "delete"])
            .current_dir(&wt_path)
            .output()
            .unwrap();

        let files = diff_for_task(root, "kernel/task-del", &base_commit).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "README.md");
        assert_eq!(files[0].status, FileStatus::Deleted);
    }

    #[test]
    fn diff_for_task_renamed_file() {
        let tmp = init_test_repo();
        let root = tmp.path();
        let (base_commit, wt_path) = create_worktree(root, "task-rename");

        Cmd::new("git")
            .args(["mv", "README.md", "DOCS.md"])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "rename"])
            .current_dir(&wt_path)
            .output()
            .unwrap();

        let files = diff_for_task(root, "kernel/task-rename", &base_commit).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "DOCS.md");
        assert_eq!(
            files[0].status,
            FileStatus::Renamed {
                from: "README.md".into()
            }
        );
    }

    #[test]
    fn diff_for_task_no_changes() {
        let tmp = init_test_repo();
        let root = tmp.path();
        let (base_commit, _wt_path) = create_worktree(root, "task-noop");

        let files = diff_for_task(root, "kernel/task-noop", &base_commit).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn diff_stat_counts() {
        let tmp = init_test_repo();
        let root = tmp.path();
        let (base_commit, wt_path) = create_worktree(root, "task-stat");

        std::fs::write(wt_path.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(wt_path.join("README.md"), "# changed\n").unwrap();
        Cmd::new("git")
            .args(["add", "."])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "changes"])
            .current_dir(&wt_path)
            .output()
            .unwrap();

        let stat = diff_stat(root, "kernel/task-stat", &base_commit).unwrap();
        assert_eq!(stat.files_changed, 2);
        assert!(stat.insertions > 0);
    }

    #[test]
    fn diff_stat_no_changes() {
        let tmp = init_test_repo();
        let root = tmp.path();
        let (base_commit, _wt_path) = create_worktree(root, "task-stat-noop");

        let stat = diff_stat(root, "kernel/task-stat-noop", &base_commit).unwrap();
        assert_eq!(stat.files_changed, 0);
        assert_eq!(stat.insertions, 0);
        assert_eq!(stat.deletions, 0);
    }

    #[test]
    fn diff_against_base_commit_not_current_head() {
        let tmp = init_test_repo();
        let root = tmp.path();
        let (base_commit, wt_path) = create_worktree(root, "task-base");

        // Make a change in the worktree
        std::fs::write(wt_path.join("file.txt"), "content\n").unwrap();
        Cmd::new("git")
            .args(["add", "."])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "add file"])
            .current_dir(&wt_path)
            .output()
            .unwrap();

        // Make a new commit on the main branch (advances HEAD of main)
        std::fs::write(root.join("main-change.txt"), "main\n").unwrap();
        Cmd::new("git")
            .args(["add", "."])
            .current_dir(root)
            .output()
            .unwrap();
        Cmd::new("git")
            .args(["commit", "-m", "main change"])
            .current_dir(root)
            .output()
            .unwrap();

        // Diff should still be against the original base_commit, not current main HEAD
        let files = diff_for_task(root, "kernel/task-base", &base_commit).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "file.txt");
    }
}
