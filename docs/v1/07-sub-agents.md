# 07 — Sub-Agent Orchestration

## Dependencies

- **02 Events** — emits agent lifecycle events (`agent_spawned`, `agent_completed`, `agent_failed`, `agent_looped`)
- **04 Compaction** — compacted context for agent creation and handoff
- **05 Modes** — each agent runs in a mode with specific tools and system prompt

## Depended on by

- 08 Tasks (tasks are executed by agents)

## Scope

The mixture-of-experts system. The root agent acts as an orchestrator that decomposes work and delegates to specialized sub-agents. This sub-system owns child-agent lifecycle, child-agent model routing, loop detection, and branching mode.

## Deliverables

### Agent Struct

```rust
enum AgentStatus {
    Spawning,
    Running,
    WaitingOnUser,
    Reporting,
    Complete,
    Failed,
}

enum AgentRole {
    Orchestrator,
    Research,
    Implementation,
    Test,
    Review,
    Unstuck,
}

struct SubAgent {
    id: Uuid,
    parent_id: Option<Uuid>,
    role: AgentRole,
    model: String,
    mode: Mode,
    status: AgentStatus,
    context: CompactedContext,
    allowed_tools: Vec<String>,
    token_usage: TokenMetrics,
}

struct TokenMetrics {
    input: u64,
    output: u64,
    cost_usd: f64,
}
```

### Orchestrator

The root agent that:

- Receives classified prompts from the prompt router
- Decomposes work into sub-agent tasks
- Spawns sub-agents with appropriate roles, models, and modes
- Collects sub-agent reports and synthesizes results
- Manages the agent tree (parent-child relationships)

### Agent Lifecycle

1. **Spawn**: Orchestrator creates sub-agent with role, model, mode, and compacted context
2. **Run**: Sub-agent executes with its mode's tools and system prompt
3. **Report**: Sub-agent produces a compacted summary of its work
4. **Complete/Fail**: Status updated, events emitted, tokens tallied

### Model Routing

- Applies only to spawned child sub-agents (not entrypoint prompts routed by 06)
- Orchestrator decides model/mode per child based on role, task state, and prior outcomes
- Each `AgentRole` has defaults from config (`models.roles.*`)
- Mode defaults and UX-agent recommendations are inputs to orchestrator policy, not direct replacements for router output
- Prompt router is not invoked for orchestrator-spawned child agents

### Loop Detection

- Monitor tool call patterns per agent
- Flag as looping if 3+ similar tool calls with similar arguments
- On loop detection:
  1. Emit `agent_looped` event
  2. Orchestrator attempts: retry with modified prompt
  3. If retry fails: spawn an "unstuck" agent with the loop context
  4. If unstuck fails: escalate to user

### Branching Mode (Parallel Exploration)

- Spawns N parallel agents working on the same prompt independently
- Each branch can use different models or temperatures
- Results collected and presented for comparison
- Must be explicitly enabled by user (expensive)
- Auto-suggest capability (UX agent can recommend branching for critical tasks)

```rust
struct BranchConfig {
    parallel_count: usize,
    models: Vec<String>,        // one per branch, or same model with different params
    auto_rank: bool,            // use eval agent to rank results
}
```

### Agent Communication

- Sub-agents report back via compacted summaries (not raw context)
- Orchestrator embeds sub-agent summaries into its own context
- Sub-agents do not communicate with each other directly — all coordination goes through the orchestrator

## Key Decisions

- Sub-agents get clean contexts (no sibling pollution)
- Agent tree is persisted in DB for UI rendering and debugging
- Cost tracking is per-agent, rolled up to per-task and per-session

## Out of Scope

- Task scheduling and dependency resolution (owned by 08 Tasks)
- Agent tree UI rendering (frontend concern)
- Individual agent steering UI (frontend concern)
