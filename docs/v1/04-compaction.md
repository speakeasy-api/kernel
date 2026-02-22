# 04 — Context Compaction

## Dependencies

- **03 Configuration** — compaction thresholds (`deep_trigger_pct`, `deep_target_pct`, `light_every_turn`)

## Depended on by

- 07 Sub-Agents (compacted context for handoff between agents)

## Scope

The compaction pipeline that runs between every agent turn. Keeps context maximally information-dense, enabling mid-run mode/model switching and long-running tasks without context degradation.

## Deliverables

### Context Budget

```rust
struct ContextBudget {
    max_tokens: usize,
    reserved_system: usize,
    reserved_response: usize,
    compaction_trigger: f32,  // e.g., 0.6
    target_after_compaction: f32, // e.g., 0.4
}
```

- Token counting for current context size
- Threshold checks to determine compaction regime

### Structural Filter (Light Compaction)

Rule-based, zero LLM cost. Runs every turn (when enabled):

- Strip thinking tags
- Collapse verbose tool outputs to key results
- Deduplicate repeated read/search patterns
- Remove redundant whitespace and formatting noise

### Semantic Compactor (Deep Compaction)

LLM-assisted. Triggered when context reaches `compaction_trigger` threshold:

- Uses the configured `compactor` model (cheap model)
- Determines if superseded logic can be removed (only the model can judge this)
- Extracts learnings from failed tool calls ("tried X, failed because Y — avoid Z")
- Summarizes completed reasoning chains while preserving decisions and rationale
- Targets `target_after_compaction` size

### Preservation Rules

The following are never removed or summarized:

- Active mode's system prompt
- All user messages in durable storage (subject to retention); active context may summarize older turns
- File paths, function signatures, error messages
- Decisions and their rationale
- Current task state and dependencies
- Extracted learnings from failures

### Compacted Context Output

```rust
struct CompactedContext {
    system_prompt: String,
    messages: Vec<Message>,      // compacted conversation
    learnings: Vec<String>,      // extracted from failures
    preserved_facts: Vec<String>, // file paths, signatures, etc.
    token_count: usize,
}
```

## Key Decisions

- Light compaction is pure Rust string processing, no LLM calls
- Deep compaction uses a single LLM call with a dedicated compaction prompt
- Superseded logic removal is never rule-based — always delegated to the compacting model
- Failed tool calls are passed to the compactor, not silently dropped

## Out of Scope

- How sub-agents use compacted context for handoff (owned by 07 Sub-Agents)
- The compaction prompt itself (designed alongside, but iteration is expected)
