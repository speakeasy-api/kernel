-- Append-only conversation message log + compaction snapshots.
-- Replaces the in-memory ConversationStore as the source of truth.

CREATE TABLE conversation_messages (
    id         TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    ordinal    INTEGER NOT NULL,
    role       TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content    TEXT NOT NULL,  -- JSON: Vec<ContentBlock>
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(session_id, ordinal)
);
CREATE INDEX idx_conv_msg_session ON conversation_messages(session_id, ordinal);

CREATE TABLE context_snapshots (
    id               TEXT PRIMARY KEY,
    session_id       TEXT NOT NULL,
    up_to_ordinal    INTEGER NOT NULL,
    summary_messages TEXT NOT NULL,  -- JSON: Vec<Message>
    created_at       DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_ctx_snap_session ON context_snapshots(session_id, created_at DESC);
