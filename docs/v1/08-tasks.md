# 08 — Task Management & Scheduler

## Dependencies

- **02 Events** — emits task lifecycle events
- **07 Sub-Agents** — tasks are executed by agents
- **09 Git Worktrees** — each task works in a dedicated worktree branch

## Depended on by

None. This is the top-level orchestration layer for long-running work.

## Scope

The task system inspired by Granary. Handles task decomposition, dependency tracking, scheduling, and the engagement level system. This is the backbone of long-running, multi-step work.

## Deliverables

### Task Struct

```rust
struct Task {
    id: Uuid,
    title: String,
    description: String,
    status: TaskStatus,
    priority: Priority,
    assigned_agent: Option<Uuid>,
    parent_task: Option<Uuid>,
    depends_on: Vec<Uuid>,
    worktree_branch: Option<String>,
    base_ref: String,
    base_commit: String,
    merge_target_ref: String,
    outcome: Option<TaskOutcome>,
}

enum TaskStatus {
    Pending,
    InProgress,
    Blocked,
    Review,
    Done,
}

enum Priority {
    Low,
    Medium,
    High,
    Critical,
}

enum TaskOutcome {
    Success { summary: String, diff_stat: DiffStat },
    Failure { reason: String },
    NeedsHuman { question: String, context: String },
}
```

### Task Decomposition

- Planning agent decomposes user's project description into a task tree
- Tasks have parent-child relationships (hierarchical breakdown)
- Dependencies are explicit edges between tasks (`task_deps` table)
- User reviews and approves the task breakdown before execution

### Scheduler

- Respects dependency graph (topological ordering)
- Picks next unblocked task(s) based on priority
- Parallelizes independent tasks up to `max_concurrent_agents`
- Each task gets its own agent with a clean context
- Async task-root agents are classified by the prompt router (same path as user entrypoint prompts)
- Assigns a dedicated worktree branch per task with explicit lineage (`base_ref`, `base_commit`, `merge_target_ref`)

### Task Lifecycle

```
User describes project
    → Planning agent decomposes into task tree
    → User reviews & approves
    → Scheduler picks next unblocked task
    → Agent assigned, works in worktree branch
        ├── Success → auto-review → merge to task.merge_target_ref → next task
        ├── Blocked → notify user, work on other tasks
        └── Failed → UX agent analyzes → retry or escalate
```

### Engagement Levels

| Level | Behavior |
|-------|----------|
| `autonomous` | Agents work through the entire task tree, only stopping for explicit blocks |
| `review-gates` | Agents pause after each task for user review before proceeding |
| `collaborative` | Agents work on one task at a time, discussing approach with user |

- Configured in `kernel.toml` (`general.engagement`)
- Can be overridden per-task

### Cost Controls

- Per-task cost tracking via agent token metrics
- Warning at `costs.warn_at_task_usd`
- Hard stop at `costs.hard_limit_task_usd`
- Per-session cost tracking (sum of all tasks)
- Warning at `costs.warn_at_usd`
- Hard stop at `costs.hard_limit_usd` — all agents halt

### Task CRUD

- `create_task(title, description, parent, deps, priority, base_ref, merge_target_ref)`
- `list_tasks(session_id, status_filter)`
- `get_task(id)`
- `update_task_status(id, status, outcome)` (persists `outcome_kind` + `outcome_data`)
- `get_task_tree(session_id)` — full tree with dependency edges
- `next_unblocked(session_id)` — scheduler query

## Key Decisions

- Task decomposition is done by a planning agent, not hard-coded rules
- The scheduler is a simple priority queue with dependency checks, not a complex job system
- Each task gets a fresh agent context — no cross-task context pollution
- Cost limits halt agents immediately, no grace period

## Out of Scope

- Task board UI (frontend concern)
- Diff review flow after task completion (frontend + 09 Git Worktrees)
