# 03 — Configuration

## Dependencies

- **01 Data Model** — for any config that touches persisted state (e.g., mode defaults)

## Depended on by

- 04 Compaction (compaction thresholds)
- 05 Modes (default model overrides)
- 10 UX Agent (engagement level, cost thresholds)

## Scope

Parsing, merging, and providing access to `kernel.toml` configuration. Handles global (`~/.config/kernel/config.toml`) and project-level (`kernel.toml` in project root) config with project overriding global.

## Deliverables

### Config Structs

Typed Rust structs matching the TOML schema:

```rust
struct KernelConfig {
    general: GeneralConfig,
    models: ModelsConfig,
    branching: BranchingConfig,
    compaction: CompactionConfig,
    costs: CostsConfig,
    retention: RetentionConfig,
}

struct GeneralConfig {
    engagement: EngagementLevel, // Autonomous | ReviewGates | Collaborative
    max_concurrent_agents: usize,
    worktree_dir: String,
}

struct ModelsConfig {
    default: String,
    prompt_router: String,
    ux_agent: String,
    compactor: String,
    roles: RoleModels,
    providers: HashMap<String, ProviderConfig>,
}

struct RoleModels {
    orchestrator: String,
    research: String,
    implementation: String,
    review: String,
    test: String,
    unstuck: String,
}

struct ProviderConfig {
    api_key_env: Option<String>,
    base_url: Option<String>,
}

struct BranchingConfig {
    enabled: bool,
    max_parallel: usize,
    auto_suggest: bool,
}

struct CompactionConfig {
    light_every_turn: bool,
    deep_trigger_pct: f32,
    deep_target_pct: f32,
}

struct CostsConfig {
    warn_at_usd: f64,
    hard_limit_usd: f64,
    warn_at_task_usd: f64,
    hard_limit_task_usd: f64,
}

struct RetentionConfig {
    raw_ttl_days: u32,          // default: 30
    stats_retention: String,    // "forever" (default); manual clear is supported
}
```

### Config Loading

- Parse TOML from both locations
- Merge project over global (field-level override)
- Validate values (e.g., `deep_trigger_pct > deep_target_pct`, `raw_ttl_days > 0`)
- Sensible defaults for all fields

### Runtime Access

- Singleton or injected config available to all sub-systems
- No hot-reloading in v1 — config is read at startup

## Key Decisions

- Use `toml` crate for parsing, `serde` for deserialization
- API keys are read from environment variables, never stored in config
- Provider config is a map to support arbitrary providers

## Out of Scope

- Model provider client implementations (owned by sub-systems that call LLMs)
- Config UI (future)
