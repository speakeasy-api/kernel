-- Unified schema for Kernel
-- Consolidates: db/migrations.rs, tasks/db.rs, ux_agent/store.rs

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    project_path TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS events (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    session_id TEXT NOT NULL,
    agent_id TEXT,
    data TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_events_session_created ON events(session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_events_kind ON events(kind);
CREATE INDEX IF NOT EXISTS idx_events_agent ON events(agent_id);

CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending',
    priority TEXT NOT NULL DEFAULT 'medium',
    assigned_agent TEXT,
    parent_task TEXT,
    worktree_branch TEXT,
    base_ref TEXT NOT NULL DEFAULT 'main',
    base_commit TEXT NOT NULL DEFAULT '',
    merge_target_ref TEXT NOT NULL DEFAULT 'main',
    outcome_kind TEXT,
    outcome_data TEXT,
    engagement_override TEXT,
    cost_usd REAL NOT NULL DEFAULT 0.0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_tasks_session_id ON tasks(session_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
CREATE INDEX IF NOT EXISTS idx_tasks_parent_task ON tasks(parent_task);

CREATE TABLE IF NOT EXISTS task_deps (
    task_id TEXT NOT NULL,
    depends_on_task_id TEXT NOT NULL,
    PRIMARY KEY (task_id, depends_on_task_id),
    FOREIGN KEY (task_id) REFERENCES tasks(id),
    FOREIGN KEY (depends_on_task_id) REFERENCES tasks(id)
);
CREATE INDEX IF NOT EXISTS idx_task_deps_task_id ON task_deps(task_id);
CREATE INDEX IF NOT EXISTS idx_task_deps_depends_on ON task_deps(depends_on_task_id);

CREATE TABLE IF NOT EXISTS agents (
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
CREATE INDEX IF NOT EXISTS idx_agents_session ON agents(session_id);
CREATE INDEX IF NOT EXISTS idx_agents_parent ON agents(parent_agent_id);

CREATE TABLE IF NOT EXISTS modes (
    name TEXT PRIMARY KEY,
    description TEXT NOT NULL,
    system_prompt TEXT NOT NULL,
    default_model TEXT,
    allowed_tools TEXT NOT NULL,
    origin TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS recommendations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trigger_pattern TEXT NOT NULL,
    recommendation TEXT NOT NULL,
    action TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
);

CREATE TABLE IF NOT EXISTS recommendation_versions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    recommendation_id INTEGER NOT NULL,
    version INTEGER NOT NULL,
    applied_at TEXT NOT NULL,
    reverted_at TEXT,
    snapshot TEXT NOT NULL,
    FOREIGN KEY (recommendation_id) REFERENCES recommendations(id)
);

CREATE TABLE IF NOT EXISTS corrections (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT,
    correction_type TEXT NOT NULL,
    original_value TEXT,
    corrected_value TEXT,
    context TEXT,
    created_at TEXT DEFAULT (datetime('now')),
    incorporated INTEGER DEFAULT 0
);

CREATE TABLE IF NOT EXISTS conventions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    convention TEXT NOT NULL,
    source_corrections TEXT NOT NULL,
    target_mode TEXT,
    status TEXT DEFAULT 'proposed',
    created_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS stats_rollups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    period_start DATETIME NOT NULL,
    period_end DATETIME NOT NULL,
    scope TEXT NOT NULL,
    scope_id TEXT,
    metric TEXT NOT NULL,
    value REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_stats_scope_metric ON stats_rollups(scope, scope_id, metric);

CREATE TABLE IF NOT EXISTS ux_agent_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_event_id TEXT,
    last_event_at TEXT,
    last_run_at TEXT
);
