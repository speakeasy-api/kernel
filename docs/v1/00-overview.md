# Kernel v1 — Sub-System Index

## Deliverables

| # | Sub-System | Dependencies |
|---|-----------|-------------|
| 01 | [Data Model & Storage](./01-data-model.md) | None |
| 02 | [Events System](./02-events.md) | 01 Data Model |
| 03 | [Configuration](./03-configuration.md) | 01 Data Model |
| 04 | [Context Compaction](./04-compaction.md) | 03 Configuration |
| 05 | [Dynamic Mode System](./05-modes.md) | 01 Data Model, 03 Configuration |
| 06 | [Prompt Router](./06-prompt-router.md) | 02 Events, 05 Modes |
| 07 | [Sub-Agent Orchestration](./07-sub-agents.md) | 02 Events, 04 Compaction, 05 Modes |
| 08 | [Task Management & Scheduler](./08-tasks.md) | 02 Events, 07 Sub-Agents, 09 Git Worktrees |
| 09 | [Git Worktree Integration](./09-git-worktrees.md) | 01 Data Model |
| 10 | [UX Agent](./10-ux-agent.md) | 02 Events, 05 Modes, 03 Configuration |

## Dependency Graph

```
01 Data Model ──────────┬──────────────────────────────┐
        │               │                              │
        ▼               ▼                              ▼
  02 Events       03 Configuration              09 Git Worktrees
        │               │                              │
        │               ├──────────┐                   │
        │               ▼          ▼                   │
        │         04 Compaction  05 Modes              │
        │               │         │ │                  │
        ├───────────────┼─────────┘ │                  │
        │               │           │                  │
        ▼               ▼           ▼                  │
  06 Prompt Router   07 Sub-Agents                     │
                        │                              │
                        ▼                              │
                  08 Tasks ◄───────────────────────────┘
        │
        ▼
  10 UX Agent (depends on 02, 03, 05)
```

## Build Order

Parallel work is possible. Recommended waves:

1. **Wave 1** (no deps): 01 Data Model, 09 Git Worktrees (partial — no DB needed for basic worktree ops)
2. **Wave 2**: 02 Events, 03 Configuration, 09 Git Worktrees (DB integration)
3. **Wave 3**: 04 Compaction, 05 Modes
4. **Wave 4**: 06 Prompt Router, 07 Sub-Agents
5. **Wave 5**: 08 Tasks, 10 UX Agent
