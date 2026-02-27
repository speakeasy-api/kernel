ALTER TABLE conversation_messages ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
ALTER TABLE conversation_messages ADD COLUMN context_snippet TEXT;
