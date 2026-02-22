# 10 — UX Agent

## Dependencies

- **02 Events** — consumes aggregated event streams via the event accumulator
- **03 Configuration** — reads/proposes changes to config values
- **05 Modes** — creates new modes, proposes edits to existing modes

## Depended on by

None. The UX agent is a background process that influences the system indirectly through recommendations and mode generation.

## Scope

The brain of Kernel's adaptive behavior. A background agent that consumes buffered, debounced event data and makes decisions about system configuration, mode creation, and user-facing recommendations. The user never interacts with it directly.

## Deliverables

### UX Agent Runtime

- Background process (not a persistent loop — wakes on trigger only)
- Triggered by the event accumulator (see 02 Events)
- Uses the configured `ux_agent` model (cheap model, runs infrequently)
- Small, focused context: aggregated event data + current config + available modes
- Uses long-term rollups for historical trends beyond raw-event retention windows
- Reads/writes persisted cursor state (`ux_agent_state`) so "since last run" is stable across restarts

### Trigger Conditions

The UX agent wakes when:

- 3+ rejection events accumulated (plan, diff, or hunk rejections)
- New session starts (review accumulated history since last run)
- Cost spike detected (sudden increase in spend rate)
- Unusual failure pattern (e.g., same tool failing repeatedly)
- User override pattern (e.g., same mode correction 3+ times)

Does NOT wake for:

- Routine success events
- No new events since last run

### Decision Types

The UX agent can produce:

1. **Mode creation** — generate a new mode based on recurring patterns
2. **Mode modification** — propose system prompt adjustments based on corrections
3. **Model recommendation** — suggest switching models for specific roles based on performance data
4. **User warning** — surface a pattern the user should know about
5. **Config change** — propose configuration adjustments

### Recommendation System

```rust
struct Recommendation {
    id: u64,
    trigger_pattern: String,
    recommendation: String,
    action: RecommendationAction,
    status: RecommendationStatus,
}

enum RecommendationAction {
    ModelChange { role: AgentRole, from_model: String, to_model: String },
    PromptEdit { mode_name: String, old_fragment: String, new_fragment: String },
    ModeCreate { name: String, description: String, system_prompt: String, default_model: Option<String>, allowed_tools: Vec<String> },
    ModeEdit { mode_name: String, changes: ModeChanges },
    ConfigChange { key: String, old_value: String, new_value: String },
}

struct ModeChanges {
    description: Option<String>,
    system_prompt: Option<String>,
    default_model: Option<String>,
    allowed_tools: Option<Vec<String>>,
}

enum RecommendationStatus {
    Pending,
    Applied,
    Dismissed,
    Reverted,
}
```

- All recommendations require user approval before application
- Applied recommendations are versioned and can be rolled back
- Dismissed recommendations are tracked (UX agent learns not to re-suggest)

### Mode Generation

When the UX agent detects a recurring pattern:

1. Analyze the pattern (e.g., 12 implementation prompts, 8 involving DB schema, 4 user overrides)
2. Draft a new mode definition (name, description, system prompt, default model, tool permissions)
3. Surface to user: "I noticed you frequently work on X. I've drafted a 'Y' mode — want to review it?"
4. User can: Review, Apply, Edit, or Dismiss

### Learning from Corrections

- Track user overrides and corrections
- Extract conventions (e.g., "always use migrations, never alter tables directly")
- Propose adding conventions to relevant mode system prompts
- Track which corrections have been incorporated and which are still pending

### UX Agent Prompt

The UX agent has a dedicated system prompt designed for:

- Pattern recognition in aggregated event data
- Mode definition generation
- Concise, actionable recommendation writing
- Conservative decision-making (suggest, don't auto-apply)

## Key Decisions

- UX agent uses a cheap model — it runs infrequently on small, aggregated data
- All outputs require user approval (no auto-apply)
- The UX agent does not see full conversation context — only aggregated event data
- Recommendations are persisted and versioned for rollback
- Historical trend detection relies on persisted stats rollups, not unbounded raw-event retention

## Out of Scope

- Recommendation UI (frontend concern — the cards with Review/Apply/Dismiss buttons)
- The event accumulator trigger logic (owned by 02 Events, but designed in coordination)
- Prompt router logic (owned by 06 Prompt Router — UX agent only influences its defaults)
