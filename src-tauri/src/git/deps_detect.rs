use std::path::Path;

use serde::{Deserialize, Serialize};

use super::diff::FileDiff;

/// Well-known dependency/lockfile names across ecosystems.
const DEPENDENCY_FILES: &[&str] = &[
    // JavaScript / Node
    "package.json",
    "package-lock.json",
    "bun.lock",
    "yarn.lock",
    // Rust
    "Cargo.toml",
    "Cargo.lock",
    // Python
    "requirements.txt",
    "pyproject.toml",
    // Go
    "go.mod",
    "go.sum",
    // Ruby
    "Gemfile",
    "Gemfile.lock",
    // Java / JVM
    "pom.xml",
    "build.gradle",
];

/// Changes split into dependency file changes and regular code changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifiedChanges {
    pub code_changes: Vec<FileDiff>,
    pub dependency_changes: Vec<FileDiff>,
}

/// Returns `true` if `path` refers to a known dependency/lockfile.
fn is_dependency_file(path: &str) -> bool {
    let filename = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);
    DEPENDENCY_FILES.contains(&filename)
}

/// Partition diffs into dependency changes and code changes.
/// Unknown files default to code changes.
pub fn classify_changes(diffs: &[FileDiff]) -> ClassifiedChanges {
    let mut code_changes = Vec::new();
    let mut dependency_changes = Vec::new();

    for diff in diffs {
        if is_dependency_file(&diff.path) {
            dependency_changes.push(diff.clone());
        } else {
            code_changes.push(diff.clone());
        }
    }

    ClassifiedChanges {
        code_changes,
        dependency_changes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::diff::{DiffLine, FileStatus, Hunk, LineKind};

    fn make_diff(path: &str) -> FileDiff {
        FileDiff {
            path: path.to_string(),
            status: FileStatus::Modified,
            hunks: vec![Hunk {
                header: "@@ -1 +1 @@".to_string(),
                lines: vec![DiffLine {
                    kind: LineKind::Add,
                    content: "change".to_string(),
                }],
            }],
        }
    }

    #[test]
    fn classifies_package_json() {
        let diffs = vec![make_diff("package.json")];
        let c = classify_changes(&diffs);
        assert_eq!(c.dependency_changes.len(), 1);
        assert!(c.code_changes.is_empty());
    }

    #[test]
    fn classifies_nested_dependency_file() {
        let diffs = vec![make_diff("packages/core/package.json")];
        let c = classify_changes(&diffs);
        assert_eq!(c.dependency_changes.len(), 1);
        assert!(c.code_changes.is_empty());
    }

    #[test]
    fn classifies_code_file() {
        let diffs = vec![make_diff("src/main.rs")];
        let c = classify_changes(&diffs);
        assert!(c.dependency_changes.is_empty());
        assert_eq!(c.code_changes.len(), 1);
    }

    #[test]
    fn unknown_files_default_to_code() {
        let diffs = vec![make_diff("README.md"), make_diff("Makefile")];
        let c = classify_changes(&diffs);
        assert!(c.dependency_changes.is_empty());
        assert_eq!(c.code_changes.len(), 2);
    }

    #[test]
    fn mixed_changes_partitioned() {
        let diffs = vec![
            make_diff("src/lib.rs"),
            make_diff("Cargo.toml"),
            make_diff("Cargo.lock"),
            make_diff("src/utils.rs"),
        ];
        let c = classify_changes(&diffs);
        assert_eq!(c.dependency_changes.len(), 2);
        assert_eq!(c.code_changes.len(), 2);
        assert_eq!(c.dependency_changes[0].path, "Cargo.toml");
        assert_eq!(c.dependency_changes[1].path, "Cargo.lock");
    }

    #[test]
    fn all_dependency_file_names_recognized() {
        let all: Vec<FileDiff> = DEPENDENCY_FILES.iter().map(|f| make_diff(f)).collect();
        let c = classify_changes(&all);
        assert_eq!(c.dependency_changes.len(), DEPENDENCY_FILES.len());
        assert!(c.code_changes.is_empty());
    }

    #[test]
    fn empty_input() {
        let c = classify_changes(&[]);
        assert!(c.code_changes.is_empty());
        assert!(c.dependency_changes.is_empty());
    }

    #[test]
    fn lockfiles_across_ecosystems() {
        let diffs = vec![
            make_diff("package-lock.json"),
            make_diff("bun.lock"),
            make_diff("yarn.lock"),
            make_diff("Gemfile.lock"),
            make_diff("go.sum"),
        ];
        let c = classify_changes(&diffs);
        assert_eq!(c.dependency_changes.len(), 5);
        assert!(c.code_changes.is_empty());
    }

    #[test]
    fn preserves_order() {
        let diffs = vec![
            make_diff("a.rs"),
            make_diff("package.json"),
            make_diff("b.rs"),
            make_diff("go.mod"),
            make_diff("c.rs"),
        ];
        let c = classify_changes(&diffs);
        assert_eq!(c.code_changes.len(), 3);
        assert_eq!(c.code_changes[0].path, "a.rs");
        assert_eq!(c.code_changes[1].path, "b.rs");
        assert_eq!(c.code_changes[2].path, "c.rs");
        assert_eq!(c.dependency_changes[0].path, "package.json");
        assert_eq!(c.dependency_changes[1].path, "go.mod");
    }
}
