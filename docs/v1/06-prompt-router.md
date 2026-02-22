# 06 — Prompt Router

## Dependencies

- **02 Events** — emits `prompt_classified`, `mode_overridden` events
- **05 Modes** — reads available modes to classify against

## Depended on by

None directly. The prompt router is the entry point for every user prompt — it feeds into whichever mode/model it selects.

## Scope

The synchronous classification layer for entrypoint prompts (user submissions and async task-root launches). Picks the best mode and model, then dispatches.

## Deliverables

### Router Input

```rust
enum PromptSource {
    User,
    AsyncTaskRoot,
}

struct RouterInput {
    source: PromptSource,            // User | AsyncTaskRoot
    prompt: String,
    available_modes: Vec<Mode>,
    conversation_history: CompactedContext,
    project_context: ProjectContext,  // language, framework, file structure hints
}
```

### Router Output

```rust
struct RouterOutput {
    mode: String,          // selected mode name
    model: String,         // selected model ID
    confidence: f32,       // 0.0–1.0
}
```

### Classification Logic

- Single LLM call using the configured `prompt_router` model
- Input: user prompt + mode list (names + descriptions only) + compacted history summary + project context
- Output: structured JSON (mode name + model ID + confidence)
- The prompt router prompt is small and focused — it sees mode metadata, not full system prompts

### User Override

- User can manually select a mode before or after classification
- Override emits a `mode_overridden` event (feeds UX agent learning)
- Override sticks for that prompt only — next prompt is re-classified

### Mid-Run Reclassification

- The router can be invoked mid-conversation when the nature of work shifts
- Triggered by: explicit user request, or root-level agent/orchestrator detecting a shift in entrypoint work
- Reclassification + mode switch is enabled by compacted context (see 04)

### Dispatch

After classification:

1. Load the selected mode's full system prompt
2. Use router-selected model for entrypoint agent creation (fallback to mode default/global default only if unavailable)
3. Emit `prompt_classified` event
4. Hand off to the agent system with mode + model + compacted context

## Key Decisions

- Uses a capable model (not the cheapest) because misclassification wastes downstream tokens
- Output is small (< 100 tokens) so cost per invocation is low despite using a stronger model
- Confidence score is logged but not acted on in v1 (future: auto-branch on low confidence)
- Prompt router owns only entrypoint routing; child sub-agent routing is owned by orchestrators (07)

## Out of Scope

- Agent execution after dispatch (owned by 07 Sub-Agents)
- Child sub-agent model/mode routing (owned by 07 Sub-Agents)
- Mode creation (owned by 10 UX Agent)
