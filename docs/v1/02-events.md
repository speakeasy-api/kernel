# 02 — Events System

## Dependencies

- **01 Data Model** — events table, agent/session foreign keys

## Depended on by

- 06 Prompt Router (emits `prompt_classified` events)
- 07 Sub-Agents (emits agent lifecycle events)
- 08 Tasks (emits task lifecycle events)
- 10 UX Agent (consumes aggregated event streams)

## Scope

The event bus — typed event emission, persistence, and querying. Every meaningful action in Kernel produces an event. This sub-system owns the event schema, emission API, and query/aggregation layer that the UX agent consumes.

## Deliverables

### Event Types

Every event carries common metadata plus variant-specific typed data. Metadata is first-class and `kind` is the discriminator for serialization/deserialization:

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

#[serde(tag = "kind", content = "data")]
enum EventData {
    // Prompt
    PromptSubmitted { prompt: String },
    PromptClassified { mode: String, model: String, confidence: f32 },
    ModeOverridden { from_mode: String, to_mode: String },

    // Agent
    AgentSpawned { agent_id: Uuid, role: AgentRole, model: String, parent_id: Option<Uuid> },
    AgentCompleted { agent_id: Uuid, summary: String, token_usage: TokenMetrics },
    AgentFailed { agent_id: Uuid, error: String, token_usage: TokenMetrics },
    AgentLooped { agent_id: Uuid, repeated_tool: String, count: u32 },
    AgentSteered { agent_id: Uuid, instruction: String },

    // Task
    TaskCreated { task_id: Uuid, title: String, parent_task: Option<Uuid> },
    TaskStarted { task_id: Uuid, agent_id: Uuid },
    TaskCompleted { task_id: Uuid, summary: String, diff_stat: DiffStat },
    TaskBlocked { task_id: Uuid, reason: String, blocked_by: Vec<Uuid> },
    TaskFailed { task_id: Uuid, reason: String },

    // Review
    PlanAccepted { task_id: Uuid },
    PlanRejected { task_id: Uuid, feedback: String },
    DiffAccepted { task_id: Uuid, branch: String },
    DiffRejected { task_id: Uuid, branch: String, feedback: String },
    HunkRejected { task_id: Uuid, file: String, hunk_index: u32, reason: String },

    // Tool
    ToolCalled { agent_id: Uuid, tool: String, args_summary: String },
    ToolSucceeded { agent_id: Uuid, tool: String, duration_ms: u64 },
    ToolFailed { agent_id: Uuid, tool: String, error: String },
    ToolRetried { agent_id: Uuid, tool: String, attempt: u32 },

    // Model
    ModelSelected { agent_id: Uuid, model: String, reason: String },
    ModelSwitchedMidRun { agent_id: Uuid, from_model: String, to_model: String, reason: String },
    TokensUsed { agent_id: Uuid, model: String, input: u64, output: u64 },
    CostIncurred { agent_id: Uuid, model: String, cost_usd: f64 },

    // Compaction
    ContextCompacted { agent_id: Uuid, before_tokens: usize, after_tokens: usize, regime: String },
    LearningExtracted { agent_id: Uuid, learning: String },
    FactsPreserved { agent_id: Uuid, facts: Vec<String> },

    // UX
    ModeCreated { mode_name: String, trigger_pattern: String },
    RecommendationSurfaced { recommendation_id: u64, summary: String },
    RecommendationApplied { recommendation_id: u64 },
    RecommendationDismissed { recommendation_id: u64 },
    WarningShown { message: String, severity: String },
}
```

### Emission API

- `emit(session_id, agent_id, data: EventData)` — write event to SQLite as `kind` + `data`
- Each `EventData` variant carries its own typed fields
- The `id` is assigned by the emission layer
- For persisted events, `EventMetadata.timestamp` is sourced from SQLite `created_at` (DB-authoritative)
- For non-persisted/ephemeral event transport, timestamp comes from a shared runtime UTC clock
- Synchronous write (SQLite is fast enough for event volume)

### Query & Aggregation Layer

The UX agent does not consume raw event streams. This sub-system provides:

- `events_since(session_id, since: DateTime)` — all events in a session after a timestamp
- `events_by_variant(variant: &str, since: DateTime)` — filter by `EventData` variant name (matches the `kind` column in SQLite)
- `aggregate_by_variant(session_id, since: DateTime)` — counts per `EventData` variant
- `rejection_rate(session_id, window: Duration)` — plan/diff rejection ratio
- `cost_total(session_id)` / `cost_total_task(task_id)` — spend aggregation
- `loop_detections(session_id, since: DateTime)` — agent loop events
- `rollup_metrics(window: Duration)` — aggregate raw events into `stats_rollups`

### Event Accumulator

Debounced trigger logic for the UX agent:

- Buffers events in memory, tracks thresholds
- Uses persisted cursor (`ux_agent_state.last_event_id` / `last_event_at`) to resume after restart
- Triggers UX agent wake-up when:
  - 3+ rejections accumulated
  - New session starts (review history since last run)
  - Cost spike detected
  - Unusual failure pattern
- Does NOT trigger on routine success events

## Key Decisions

- Events are append-only and never mutated in place; retention may delete expired raw rows
- `EventData` variants are extensible — new variants can be added without migration (the SQLite `kind` column stores the variant name as text, and `data` stores the variant's fields as JSON)
- Type safety lives in the Rust layer; the DB stores a denormalized `kind` for efficient querying
- The accumulator is an in-memory buffer, not a separate process; recovery uses persisted cursors
- Aggregation queries run on SQLite indexes, not in-memory
- Raw events are retained for 30 days; long-term analytics use persisted rollups
- Persisted event ordering/timestamps are derived from DB rows, not caller-provided wall-clock values

## Out of Scope

- What the UX agent does with the events (owned by 10 UX Agent)
- Individual event emission call sites (owned by the sub-system that produces the event)
