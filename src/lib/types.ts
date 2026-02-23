// Mirrors db/models.rs

export interface Session {
  id: string;
  project_path: string;
  created_at: string;
}

export interface RawEvent {
  id: string;
  kind: string;
  session_id: string;
  agent_id: string | null;
  data: string;
  created_at: string;
}

export interface Agent {
  id: string;
  session_id: string;
  parent_agent_id: string | null;
  task_id: string | null;
  role: string;
  model: string;
  mode: string;
  status: string;
  token_input: number;
  token_output: number;
  created_at: string;
  finished_at: string | null;
}

export interface DbMode {
  name: string;
  description: string;
  system_prompt: string;
  default_model: string | null;
  allowed_tools: string;
  origin: string;
  version: number;
  created_at: string;
  updated_at: string;
}

// Mirrors modes/types.rs

export type ModeOrigin = "builtin" | "ux_agent" | "user";

export interface Mode {
  name: string;
  description: string;
  system_prompt: string;
  default_model: string | null;
  allowed_tools: string[];
  created_by: ModeOrigin;
  version: number;
}

// Mirrors config/types.rs

export type EngagementLevel = "autonomous" | "review_gates" | "collaborative";

export interface KernelConfig {
  general: {
    engagement: EngagementLevel;
    max_concurrent_agents: number;
    worktree_dir: string;
  };
  models: {
    default: string;
    prompt_router: string;
    ux_agent: string;
    compactor: string;
    roles: {
      orchestrator: string;
      research: string;
      implementation: string;
      review: string;
      test: string;
      unstuck: string;
    };
    providers: Record<
      string,
      { api_key_env: string | null; base_url: string | null }
    >;
  };
  branching: {
    enabled: boolean;
    max_parallel: number;
    auto_suggest: boolean;
  };
  compaction: {
    light_every_turn: boolean;
    deep_trigger_pct: number;
    deep_target_pct: number;
  };
  costs: {
    warn_at_usd: number;
    hard_limit_usd: number;
    warn_at_task_usd: number;
    hard_limit_task_usd: number;
  };
  retention: {
    raw_ttl_days: number;
    stats_retention: string;
  };
}
