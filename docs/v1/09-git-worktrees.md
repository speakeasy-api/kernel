# 09 — Git Worktree Integration

## Dependencies

- **01 Data Model** — task/branch association storage

## Depended on by

- 08 Tasks (each task works in a dedicated worktree)

## Scope

All git worktree operations. The user's working tree is never touched directly — all Kernel work happens in worktrees under `.kernel/worktrees/`.

## Deliverables

### Worktree Management

```rust
struct Worktree {
    path: PathBuf,
    branch: String,
    task_id: Option<Uuid>,
    base_ref: String,
    base_commit: String,
    merge_target_ref: String,
    created_at: DateTime,
}
```

- `create_worktree(task_id, branch_name, base_ref, merge_target_ref)` — creates `.kernel/worktrees/<branch>/` from `base_ref` and records resolved `base_commit`
- `remove_worktree(branch_name)` — clean up after merge or abandonment
- `list_worktrees()` — all active worktrees
- `worktree_for_task(task_id)` — resolve task to its worktree path

### Branch Naming

Convention: `kernel/<task-slug>` (e.g., `kernel/task-auth-middleware`)

Nested flow is supported:

- Initiative/epic branch (e.g., `kernel/initiative-auth`) can be a task's `base_ref`
- Child task branches merge back into initiative branch (`merge_target_ref = kernel/initiative-auth`)
- Initiative branch later merges into `main` or a feature branch

### Diff Generation

- `diff_for_task(task_id)` — generate diff between task branch HEAD and persisted `base_commit`
- `diff_stat(task_id)` — summary stats (files changed, insertions, deletions)
- Per-file diffs with syntax-aware context

### Merge Flow

```
Task complete
    → Review agent generates diff summary
    → User opens diff viewer
        ├── Accept all → squash merge to `task.merge_target_ref`
        ├── Partial accept → accepted hunks merged, rejected hunks create follow-up task
        └── Reject all → task marked for rework
```

- `merge_to_target(task_id, strategy)` — squash/merge/cherry-pick into persisted `merge_target_ref`
- `partial_merge(task_branch, accepted_hunks)` — apply selected hunks only
- Rejected hunks are captured and fed back as a follow-up task

### Dependency Change Detection

- Detect changes to dependency files (`package.json`, `Cargo.toml`, `requirements.txt`, etc.)
- Flag dependency changes distinctly in diff output
- Support independent approve/reject for dependency changes vs. code changes

### Cleanup

- Remove worktrees after successful merge
- Garbage collection for abandoned worktrees (no associated active task)

## Key Decisions

- All worktrees live under `.kernel/worktrees/` — single location, easy to clean up
- Squash merge is the default strategy (clean history)
- The user's working tree is strictly read-only from Kernel's perspective
- Partial merges create follow-up tasks automatically
- Task lineage is explicit and immutable after creation (`base_ref`, `base_commit`, `merge_target_ref`)

## Out of Scope

- Diff viewer UI (frontend concern)
- Inline comment system (frontend concern)
- Conflict resolution UI (frontend concern, v1 may just escalate conflicts to user)
