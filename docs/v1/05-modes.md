# 05 — Dynamic Mode System

## Dependencies

- **01 Data Model** — `modes` table for persistence
- **03 Configuration** — default model overrides per mode

## Depended on by

- 06 Prompt Router (selects among available modes)
- 07 Sub-Agents (each agent runs in a mode)
- 10 UX Agent (creates/modifies modes)

## Scope

Mode definitions, storage, retrieval, and the interface for creating new modes. Modes are not hard-coded — Kernel ships with base modes, but the UX agent can generate new ones at runtime.

## Deliverables

### Mode Struct

```rust
struct Mode {
    name: String,
    description: String,
    system_prompt: String,
    default_model: Option<String>,
    allowed_tools: Vec<String>,
    created_by: ModeOrigin,
    version: u32,
}

enum ModeOrigin {
    BuiltIn,
    UXAgent,
    User,
}
```

### Base Modes

Ship with these built-in modes:

| Mode | Focus | Tools |
|------|-------|-------|
| Plan | Structured decomposition, trade-off analysis, step-by-step reasoning | Read-only |
| Implement | Code generation, tool-use heavy, minimal explanation | Full access |
| Review | Diff-aware, security & correctness focus, concise feedback | Read-only + git |
| Debug | Hypothesis-driven, log analysis, bisect strategy | Full access |
| Research | Deep reading, summarization, web search | Read-only + web |
| General | Balanced, conversational | Full access |

Each base mode includes a complete system prompt (these will be iterated on).

### Mode CRUD

- `list_modes()` — all available modes (builtin + generated + user)
- `get_mode(name)` — single mode by name
- `create_mode(mode)` — insert new mode (used by UX agent)
- `update_mode(name, changes)` — modify existing mode, increment version
- `delete_mode(name)` — only for non-builtin modes

### Mode Files

Modes are also stored as files (editable by the user):

- Location: `.kernel/modes/` in project root
- Format: TOML or Markdown with frontmatter
- File-based modes sync to DB on startup
- User edits to files take precedence over DB

### Tool Permission Sets

Define named tool groups that modes reference:

- `read_only`: `fs_read`, `glob`, `grep`
- `read_write`: `fs_read`, `fs_write`, `glob`, `grep`
- `full`: `fs_read`, `fs_write`, `glob`, `grep`, `shell`, `git`
- `web`: `web_search`, `web_fetch`

Modes combine these (e.g., Review = `read_only` + `git`).

## Key Decisions

- Builtin modes cannot be deleted, only overridden
- Mode versions are tracked for rollback
- System prompts are full text, not templates — no variable interpolation

## Out of Scope

- Mode generation logic (owned by 10 UX Agent)
- Mode selection logic (owned by 06 Prompt Router)
- System prompt content design (iterative, not a deliverable gate)
