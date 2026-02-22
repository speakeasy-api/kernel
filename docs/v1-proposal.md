# Kernel v1 — Technical Proposal

## Overview

Kernel is a standalone LLM code tool that runs as a native desktop app. Unlike CLI-based tools like Claude Code and Codex, Kernel provides a rich visual interface for orchestrating multiple AI agents across long-running, complex software engineering tasks.

The core thesis: a single model in a single context window is a poor match for real-world engineering work. Kernel treats LLM orchestration as a first-class systems problem — routing prompts to the right model under the right persona, distributing work across sub-agents, compacting context on the fly, and learning from its own failures over time.

What sets Kernel apart is that it **learns**. It learns the user's patterns, the models' strengths and weaknesses, and the project's conventions — then adapts its behavior continuously. This is not hard-coded heuristics. It's an agent observing an event stream and making decisions.

---

## The Brain: Prompt Router + UX Agent

Kernel's intelligence is split into two distinct concerns that operate on different timescales:

### Prompt Router

The prompt router runs **synchronously on every entrypoint prompt submission** (user prompts and async task-root launches). It is the hot path. Its job: classify the prompt into a mode and pick the best model for initial execution.

```
User submits prompt
        │
        ▼
┌─────────────────────────────────┐
│         Prompt Router           │
│                                 │
│  Input:                         │
│  - User prompt                  │
│  - Available modes              │
│  - Conversation history (compact│ed)
│  - Project context              │
│                                 │
│  Output:                        │
│  - Mode selection               │
│  - Model selection              │
│  - Confidence score             │
└─────────────────────────────────┘
        │
        ▼
Prompt dispatched to selected mode + model
```

The prompt router should use a **capable model** — it's making a decision that shapes the entire downstream execution. Getting this wrong means wasted tokens and time. It sees the prompt, the list of available modes (including UX-agent-generated ones), compacted conversation history, and project context. Its output is small (mode name + model ID + confidence), so despite using a stronger model, the cost per invocation is low.

### UX Agent

The UX agent is the brain of Kernel's adaptive behavior. It is a background agent that the user never interacts with directly. Unlike the prompt router, it does **not** run on every event. It consumes a buffered, debounced stream of events and wakes up only when there's something worth responding to.

```
┌──────────────────────────────────────────────────┐
│                   Event Bus                      │
│                                                  │
│  prompt_classified  mode_overridden  plan_rejected│
│  tool_failed  loop_detected  task_completed      │
│  review_rejected  cost_spike  model_switched     │
│  user_override  compaction_ran  agent_spawned    │
│  ...any event the system produces                │
└──────────────┬───────────────────────────────────┘
               │
               ▼
┌──────────────────────────────────────────────────┐
│              Event Accumulator                   │
│                                                  │
│  Buffers events. Debounces. Triggers UX agent    │
│  only when:                                      │
│                                                  │
│  - A threshold is crossed (e.g., 3+ rejections)  │
│  - A new session starts (review accumulated      │
│    history since last run)                        │
│  - An anomaly is detected (cost spike, unusual   │
│    failure pattern)                               │
│                                                  │
│  Does NOT trigger if:                            │
│  - No new events since last run                  │
│  - Only routine events (successful completions)  │
└──────────────┬───────────────────────────────────┘
               │ (only when triggered)
               ▼
┌──────────────────────────────────────────────────┐
│                  UX Agent                        │
│                                                  │
│  Decisions:                                      │
│  - Should a new mode be created for this         │
│    recurring pattern?                            │
│  - Should the user be warned about something?    │
│  - Should model/mode defaults be adjusted?       │
│  - Has the user's rejection pattern revealed     │
│    a gap in a system prompt?                     │
│  - Is branching appropriate for certain tasks?   │
│                                                  │
│  Outputs:                                        │
│  - User-facing warnings/recommendations          │
│  - New mode definitions (generated on the fly)   │
│  - System prompt adjustments                     │
│  - Configuration changes (with user approval)    │
└──────────────────────────────────────────────────┘
```

The UX agent's context is small and focused: it receives aggregated event data, current configuration, and the list of available modes/models. It does not see full conversation context. If there are no events worth responding to, it simply doesn't run.

### Events System

Events are the nervous system of Kernel. Every meaningful action produces an event. Events are structured, typed, and stored persistently.

```rust
struct EventMetadata {
    id: Uuid,
    timestamp: DateTime,
    session_id: Uuid,
    agent_id: Option<Uuid>,
}

struct Event {
    metadata: EventMetadata,
    #[serde(flatten)]
    data: EventData,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
enum EventData {
    PromptSubmitted { prompt: String },
    PromptClassified { mode: String, model: String, confidence: f32 },
    // ... extensible
}
```

Timestamp contract:

- For persisted events, `EventMetadata.timestamp` is sourced from SQLite `events.created_at` (DB is authoritative)
- For non-persisted/ephemeral event transport, timestamp is generated by a shared UTC time source in the runtime

Event kinds include (but are not limited to — the system is extensible):

| Category | Events |
|----------|--------|
| Prompt | `prompt_submitted`, `prompt_classified`, `mode_overridden` |
| Agent | `agent_spawned`, `agent_completed`, `agent_failed`, `agent_looped`, `agent_steered` |
| Task | `task_created`, `task_started`, `task_completed`, `task_blocked`, `task_failed` |
| Review | `plan_accepted`, `plan_rejected`, `diff_accepted`, `diff_rejected`, `hunk_rejected` |
| Tool | `tool_called`, `tool_succeeded`, `tool_failed`, `tool_retried` |
| Model | `model_selected`, `model_switched_mid_run`, `tokens_used`, `cost_incurred` |
| Compaction | `context_compacted`, `learning_extracted`, `facts_preserved` |
| UX | `mode_created`, `recommendation_surfaced`, `recommendation_applied`, `recommendation_dismissed`, `warning_shown` |

Events are stored in SQLite and are queryable. The UX agent receives filtered, aggregated views — not raw event streams — to keep its context small and decisions fast.

---

## Dynamic Mode System

Modes are not hard-coded. Kernel ships with a set of well-defined base modes, but the UX agent can generate new modes on the fly. The prompt router selects among all available modes (built-in and generated) on every prompt.

### Base Modes

Each mode is a self-contained definition:

```rust
struct Mode {
    name: String,
    description: String,
    system_prompt: String,
    default_model: Option<String>,  // override default model
    allowed_tools: Vec<String>,    // e.g. ["fs_read", "fs_write", "shell", "git", "web"]
    created_by: ModeOrigin,  // BuiltIn | UXAgent | User
}
```

Initial base modes:

| Mode | Focus |
|------|-------|
| Plan | Structured decomposition, trade-off analysis, step-by-step reasoning. Read-only tools. |
| Implement | Code generation, tool-use heavy, minimal explanation. Full tool access. |
| Review | Diff-aware, security & correctness focus, concise feedback. Read-only + git. |
| Debug | Hypothesis-driven, log analysis, bisect strategy. Full tool access. |
| Research | Deep reading, summarization, web search. Read-only + web. |
| General | Balanced, conversational. Full access. |

### Mode Generation

The UX agent has a prompt designed to help it generate new modes. When it detects a recurring pattern — e.g., the user frequently asks for database migration help, or repeatedly works on API contract design — it can create a new mode:

```
UX Agent observes:
  - 12 prompts in the last week classified as "Implement"
  - 8 of those involved database schema changes
  - User overrode to add specific instructions 4 times
  - Common corrections: "always use migrations, never alter tables directly"

UX Agent generates:
  Mode: "Database"
  System prompt: [tailored for migration-first schema work,
                   includes project conventions extracted from corrections]
  Default model: [whatever performed best on these tasks]
  Tool permissions: Full FS + shell + git

UX Agent surfaces:
  "I noticed you frequently work on database schema changes.
   I've drafted a 'Database' mode — want to review it?"
   → [Review] [Apply] [Dismiss]
```

Mode definitions are stored as files (editable by the user) and versioned. The UX agent can propose changes to existing modes too — e.g., adjusting a system prompt based on accumulated corrections.

### Mid-Run Mode and Model Switching

Because context is continuously compacted (see section 6), switching modes or models mid-run is practical. For entrypoint work, the prompt router can switch mode/model mid-conversation. For spawned child agents, the orchestrator controls model/mode selection independently. The new agent receives:

1. The compacted history (high-signal summary of everything that happened)
2. The new mode's system prompt
3. The current task state

This means the prompt router can decide mid-conversation: "this started as planning but has shifted to implementation" — and seamlessly hand off with full continuity.

---

## Mixture of Experts (Sub-Agent Orchestration)

The root agent acts as an orchestrator. It does not do heavy lifting directly. Instead, it decomposes work and delegates to sub-agents.

```
Root Agent (orchestrator)
├── Research Agent      → gathers context, reads files, searches
├── Implementation Agent → writes code in worktree
├── Test Agent          → runs tests, analyzes failures
├── Review Agent        → reviews implementation diff
└── Unstuck Agent       → spawned on loop detection
```

### Sub-Agent Lifecycle

```rust
enum AgentStatus {
    Spawning,
    Running,
    WaitingOnUser,
    Reporting,
    Complete,
    Failed,
}

struct SubAgent {
    id: Uuid,
    parent_id: Option<Uuid>,
    role: AgentRole,
    model: ModelConfig,
    mode: Mode,
    status: AgentStatus,
    context: CompactedContext,
    allowed_tools: Vec<String>,
    token_usage: TokenMetrics,
}
```

Model routing for spawned child agents is configurable per agent role and controlled by the orchestrator. The UX agent can also override these defaults based on learned performance data.

### UI/UX for Sub-Agents

This is the most critical UX challenge in the product:

1. **Agent Tree Panel** — persistent sidebar showing root → sub-agent hierarchy. Each node: role icon, model badge, status indicator, token count. Click to open that agent's conversation.

2. **Inline annotations** — when root spawns a sub-agent, an inline card appears in the main chat. When a sub-agent reports back, the summary appears with a drill-down affordance. Collapsible.

3. **Individual agent steering** — click into any sub-agent to see its conversation. Inject messages, cancel, restart, or redirect. Changes propagate upstream.

4. **Status bar** — aggregate view: active agent count, total tokens, estimated cost.

### Loop Detection

If an agent repeats a tool call pattern 3+ times with similar arguments, it's flagged as looping. The orchestrator can: retry with a different prompt, spawn an "unstuck" expert agent, or escalate to the user. This event feeds the UX agent's learning.

---

## Self-Healing

### Introspection via Events

The UX agent consumes outcome events and detects patterns:

| Signal | Source |
|--------|--------|
| Plan rejected | User rejects a plan |
| Code review failure | Review agent flags issues; or user requests changes |
| Test failure | Test agent reports failing tests after implementation |
| Loop detected | Agent loops without progress |
| User override | User manually corrects agent output |
| Mode mismatch | User switches mode after UX agent chose wrong one |

The UX agent operates on aggregated data, not raw context. Example outputs:

> **Pattern detected:** Plan rejection rate is 60% when using haiku for planning.
> **Recommendation:** Switch planning model to sonnet → [Apply] [Dismiss]

> **Pattern detected:** Implementation agent loops frequently on test setup tasks.
> **Recommendation:** Add to mode prompt: "When setting up tests, always check for existing test utilities in `src/test/helpers`" → [Apply] [Edit] [Dismiss]

> **Pattern detected:** You've corrected the agent on error handling strategy 3 times this week.
> **Recommendation:** Add to project configuration: "Use Result types for all fallible operations. Never use unwrap in production code." → [Apply] [Edit] [Dismiss]

Applied recommendations are versioned. Users can roll back any change.

### Branching Mode (Parallel Exploration)

For high-stakes tasks (planning, architecture, scaffolding), branching mode spawns N parallel agents working on the same prompt independently.

```
User prompt: "Design the authentication system"
                    │
         ┌──────────┼──────────┐
         ▼          ▼          ▼
    Agent A     Agent B     Agent C
    (opus)     (sonnet)    (opus, different temp)
         │          │          │
         ▼          ▼          ▼
    Result A    Result B    Result C
                    │
              ┌─────┴─────┐
              ▼           ▼
         Auto-rank    User review
         (by eval     (side-by-side
          agent)       comparison)
```

This must be explicitly enabled by the user (it's expensive). The UX agent can suggest branching for critical tasks based on learned patterns. Metrics from all branches feed back into model selection optimization.

---

## Async & Task Management

Inspired by [Granary](https://github.com/speakeasy-api/granary), Kernel's task system is the backbone of long-running work.

```rust
struct Task {
    id: Uuid,
    title: String,
    description: String,
    status: TaskStatus,          // Pending, InProgress, Blocked, Review, Done
    priority: Priority,
    assigned_agent: Option<Uuid>,
    parent_task: Option<Uuid>,
    depends_on: Vec<Uuid>,
    worktree_branch: Option<String>,
    base_ref: String,            // ref used to create task branch (e.g., feature/x or parent task branch)
    base_commit: String,         // resolved SHA at creation for stable diffs
    merge_target_ref: String,    // where approved changes are merged
    outcome: Option<TaskOutcome>,
}

enum TaskOutcome {
    Success { summary: String, diff_stat: DiffStat },
    Failure { reason: String },
    NeedsHuman { question: String, context: String },
}
```

### Task Lifecycle

```
User describes project
        │
        ▼
Planning agent decomposes into task tree
        │
        ▼
User reviews & approves task breakdown
        │
        ▼
Scheduler picks next unblocked task
        │
        ▼
Agent assigned → works in dedicated worktree branch
        │
        ├── Success → auto-review → merge to task.merge_target_ref → next task
        ├── Blocked → notify user, work on other tasks
        └── Failed → UX agent analyzes → retry or escalate
```

### Scheduler

- Respects dependency graph (topological ordering)
- Parallelizes independent tasks across agents (up to configured concurrency)
- Each task gets its own agent with a clean context (no pollution from siblings)
- Task events feed the UX agent for continuous learning

### Engagement Levels

| Level | Behavior |
|-------|----------|
| `autonomous` | Agents work through the entire task tree, only stopping for explicit blocks |
| `review-gates` | Agents pause after each task for user review before proceeding |
| `collaborative` | Agents work on one task at a time, discussing approach with user |

### UI: Task Board

Kanban-style board (Pending → In Progress → In Review → Done). Each card shows title, priority, assigned agent + model, branch name, sub-task progress. Click to drill into agent conversation and diff.

---

## Git Worktree Native

All Kernel work happens in git worktrees. The user's working tree is never touched directly.

```
main (user's branch, untouched)
└── kernel/initiative-auth (epic branch + worktree)
    ├── kernel/task-auth-middleware
    ├── kernel/task-session-store
    └── kernel/task-oauth-flow
        (children merge into kernel/initiative-auth after review)
        (initiative branch later merges into main or feature branch)
        
```

Task branches are lineage-aware, not "whatever HEAD is now":

```
task.branch            = kernel/task-<slug>
task.base_ref          = logical source ref/branch at creation
task.base_commit       = resolved commit SHA of base_ref at creation
task.merge_target_ref  = ref that accepted changes merge into
```

```
main / feature (integration target)
├── .kernel/worktrees/
│   ├── task-auth-middleware/     → kernel/task-auth-middleware branch
│   ├── task-db-migration/       → kernel/task-db-migration branch
│   └── task-api-routes/         → kernel/task-api-routes branch
```

### Dependency Handling

When a task installs new dependencies, a review agent inspects the change. Dependency changes are flagged distinctly in the diff view. User can approve/reject dependency changes independently of code changes.

### Diff Viewer

Core UI component. Requirements:

1. **Side-by-side and unified views** — toggle between them
2. **Syntax highlighting** — language-aware rendering
3. **Inline comments** — user comments on lines feed back to the agent as review
4. **File tree** — changed files with +/- line counts
5. **Chunk-level accept/reject** — accept some hunks, reject others; rejected hunks become follow-up work
6. **Merge controls** — merge to task target ref, squash, or cherry-pick

### Merge Flow

```
Task complete
    │
    ▼
Review agent generates diff summary
    │
    ▼
User opens diff viewer
    │
    ├── Accept all → squash merge to task.merge_target_ref
    ├── Partial accept → accepted hunks merged, rejected hunks create follow-up task
    └── Reject all → task marked for rework, agent retries with feedback
```

---

## Context Compaction

Context compaction runs between every turn. The goal is to keep the context maximally information-dense at all times. This is what enables mid-run mode/model switching and long-running tasks without context degradation.

### Compaction Pipeline

```
Raw context (after agent turn)
        │
        ▼
    ┌─────────────────────┐
    │ Structural Filter   │  ← Rule-based, zero LLM cost
    │                     │
    │ - Strip thinking tags
    │ - Collapse verbose tool outputs to key results
    │ - Deduplicate repeated read/search patterns
    └─────────────────────┘
        │
        ▼
    ┌─────────────────────┐
    │ Semantic Compactor   │  ← Cheap LLM call
    │                     │
    │ Receives the structurally filtered context.
    │ Its job:
    │
    │ - Determine if superseded logic can be removed
    │   (only the model can judge this — what looks
    │    superseded may contain context needed later)
    │
    │ - Analyze failed tool calls: extract learnings
    │   that should be embedded into the next turn
    │   ("tried X, failed because Y — avoid Z")
    │
    │ - Summarize completed reasoning chains while
    │   preserving decisions and their rationale
    │
    │ - Preserve: user instructions, file paths,
    │   function signatures, error messages, task state
    └─────────────────────┘
        │
        ▼
    Compacted context (fed to next turn)
```

### Key Design Decisions

**Superseded logic cannot be removed programmatically.** A rule-based system cannot reliably determine when old logic is truly superseded vs. when it contains context that will matter later. Only the compacting model can make this judgment — it sees the full picture and can reason about what's still relevant.

**Failed tool calls are not simply discarded.** They are passed to the compacting model, which determines if there's a learning to embed into the next turn. A failed `grep` that found nothing might mean "this pattern doesn't exist in the codebase" — a fact worth carrying forward. A failed shell command might reveal an environment constraint. The compacting model extracts these learnings and embeds them as concise statements in the compacted context.

### What is Always Preserved

- The active mode's system prompt
- All user messages are preserved in durable storage (subject to retention policy); active context may summarize older messages
- File paths, function signatures, error messages (factual anchors)
- Decisions and their rationale
- Current task state and dependencies
- Extracted learnings from failures

### Token Budget

```rust
struct ContextBudget {
    max_tokens: usize,
    reserved_system: usize,
    reserved_response: usize,
    compaction_trigger: f32,     // trigger deep compaction at this % (e.g., 0.6)
    target_after_compaction: f32, // compact down to this % (e.g., 0.4)
}
```

Two regimes:
1. **Light compaction** (every turn): structural filtering only, zero LLM cost
2. **Deep compaction** (at trigger threshold): LLM-assisted semantic compaction with learning extraction

### Compaction Enables Everything Else

Continuous compaction is not just a performance optimization — it's what makes the rest of the system work:

- **Mid-run mode switching**: new mode agent gets compacted history, not 50 turns of raw conversation
- **Mid-run model switching**: smaller model can take over from larger one because context is lean
- **Long-running tasks**: a 200-turn task performs like a 10-turn one
- **Sub-agent handoff**: when a sub-agent reports back, its compacted summary is clean enough to embed in the root agent's context without pollution

---

## Data Model

### SQLite Schema

```sql
-- Events (the nervous system)
CREATE TABLE events (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    session_id TEXT NOT NULL,
    agent_id TEXT,
    data TEXT NOT NULL,  -- JSON payload for EventData variant fields
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_events_kind ON events(kind);
CREATE INDEX idx_events_session ON events(session_id);

-- Sessions
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    project_path TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Tasks
CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    parent_id TEXT REFERENCES tasks(id),
    title TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    priority INTEGER NOT NULL DEFAULT 2,
    worktree_branch TEXT,
    base_ref TEXT NOT NULL,
    base_commit TEXT NOT NULL,
    merge_target_ref TEXT NOT NULL,
    outcome_kind TEXT,    -- 'success' | 'failure' | 'needs_human'
    outcome_data TEXT,    -- JSON payload for TaskOutcome fields
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    completed_at DATETIME
);

CREATE TABLE task_deps (
    task_id TEXT NOT NULL REFERENCES tasks(id),
    depends_on TEXT NOT NULL REFERENCES tasks(id),
    PRIMARY KEY (task_id, depends_on)
);

-- Agents
CREATE TABLE agents (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    parent_agent_id TEXT REFERENCES agents(id),
    task_id TEXT REFERENCES tasks(id),
    role TEXT NOT NULL,
    model TEXT NOT NULL,
    mode TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'spawning',
    token_input INTEGER DEFAULT 0,
    token_output INTEGER DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    finished_at DATETIME
);

-- Modes (including UX-agent-generated ones)
CREATE TABLE modes (
    name TEXT PRIMARY KEY,
    description TEXT NOT NULL,
    system_prompt TEXT NOT NULL,
    default_model TEXT,
    allowed_tools TEXT NOT NULL,     -- JSON array of tool names
    origin TEXT NOT NULL,            -- 'builtin', 'ux_agent', 'user'
    version INTEGER NOT NULL DEFAULT 1,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- UX Agent recommendations
CREATE TABLE recommendations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trigger_pattern TEXT NOT NULL,   -- what events triggered this
    recommendation TEXT NOT NULL,
    action_type TEXT NOT NULL,       -- 'model_change', 'prompt_edit', 'mode_create', 'mode_edit', 'config_change'
    action_payload TEXT NOT NULL,    -- JSON
    status TEXT NOT NULL DEFAULT 'pending',  -- 'pending', 'applied', 'dismissed', 'reverted'
    applied_at DATETIME,
    reverted_at DATETIME,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Aggregated stats (retained indefinitely unless manually cleared)
CREATE TABLE stats_rollups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    period_start DATETIME NOT NULL,
    period_end DATETIME NOT NULL,
    scope TEXT NOT NULL,       -- 'session', 'task', 'global'
    scope_id TEXT,             -- nullable for global stats
    metric TEXT NOT NULL,      -- e.g., 'events.prompt_classified', 'cost.usd'
    value REAL NOT NULL
);
CREATE INDEX idx_stats_scope_metric ON stats_rollups(scope, scope_id, metric);

-- UX agent cursor/state (for reliable "since last run" semantics)
CREATE TABLE ux_agent_state (
    scope TEXT PRIMARY KEY,      -- 'global' or session_id
    last_event_id TEXT,
    last_event_at DATETIME,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### Retention & Eviction

- Raw operational data (`events`, conversation transcripts, tool outputs, agent logs) is retained for 30 days
- Aggregated stats (`stats_rollups`) are retained indefinitely unless manually cleared
- Eviction runs only after rollups are persisted, so historical metrics survive raw-data cleanup
- Manual clear supports raw-only, stats-only, or full wipe

### Configuration (kernel.toml)

Project root (project-level) or `~/.config/kernel/config.toml` (global). Project overrides global.

```toml
[general]
engagement = "review-gates"    # autonomous | review-gates | collaborative
max_concurrent_agents = 4
worktree_dir = ".kernel/worktrees"

[models]
default = "claude-sonnet-4-20250514"
prompt_router = "claude-sonnet-4-20250514"  # capable — shapes downstream execution
ux_agent = "claude-haiku-4-5-20251001"      # cheap — runs infrequently on aggregated events
compactor = "claude-haiku-4-5-20251001"

[models.roles]
orchestrator = "claude-opus-4-20250514"
research = "claude-haiku-4-5-20251001"
implementation = "claude-sonnet-4-20250514"
review = "claude-opus-4-20250514"
test = "claude-haiku-4-5-20251001"
unstuck = "claude-opus-4-20250514"

[models.providers.anthropic]
api_key_env = "ANTHROPIC_API_KEY"

[models.providers.openai]
api_key_env = "OPENAI_API_KEY"

[models.providers.openrouter]
api_key_env = "OPENROUTER_API_KEY"

[models.providers.ollama]
base_url = "http://localhost:11434"  # no API key needed

[branching]
enabled = false
max_parallel = 3
auto_suggest = true

[compaction]
light_every_turn = true
deep_trigger_pct = 0.6
deep_target_pct = 0.4

[costs]
warn_at_usd = 5.0       # surface a warning at this spend per session
hard_limit_usd = 20.0   # halt all agents at this spend per session
warn_at_task_usd = 1.0  # per-task warning threshold
hard_limit_task_usd = 5.0  # per-task hard limit

[retention]
raw_ttl_days = 30
stats_retention = "forever"  # retained indefinitely; manual clear is supported
```

---

## Resolved Decisions

1. **Sandboxing:** No formal sandboxing (no Docker/nsjail). Agents get `allowedTools` configuration per role. Tauri's permission system provides the safety net for v1.

2. **MCP support:** Deferred past v1. MCP servers are just another tool — easy to add later without architectural changes.

3. **Cost controls:** Enforced at two levels — per-session and per-task. Configurable warning thresholds and hard limits. When a hard limit is hit, all agents halt and the user is notified.

4. **Prompt routing vs. UX maintenance:** Split into two agents. Prompt router runs synchronously on every prompt (capable model, small output). UX agent runs asynchronously, debounced, only when accumulated events cross a threshold. UX agent does not run if there are no new events.

5. **Routing ownership boundaries:** Prompt router selects mode/model for entrypoint prompts (user submissions and async task-root launches). Orchestrators select model/mode for spawned child sub-agents.

---

## Success Criteria

- User can open a project, describe a multi-file change, and have Kernel plan → implement → test → review it across multiple agents with minimal babysitting
- All work happens in worktrees; user reviews and merges via the built-in diff viewer
- Context stays clean — a 50-turn conversation should perform as well as a 5-turn one
- The prompt router correctly classifies mode > 80% of the time without user override
- After a week of active use, the UX agent has generated at least one new mode or meaningful recommendation
- Sub-agent tree is legible — user can always understand what's happening and intervene
- Mid-run mode/model switching is seamless — user doesn't notice the handoff
