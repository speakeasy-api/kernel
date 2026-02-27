CREATE TABLE session_plans (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id),
    plan_filename TEXT NOT NULL,
    attached_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
