# 01 — Data Model & Storage

## Dependencies

None. This is a foundational sub-system.

## Depended on by

- 02 Events
- 03 Configuration
- 05 Modes
- 09 Git Worktrees

## Scope

SQLite database setup, schema management, and data access layer. This sub-system owns the persistent storage for all other sub-systems.

## Deliverables

### Schema

All tables from the proposal:

- `sessions` — project sessions
- `events` — event log (the nervous system); stores `kind TEXT` (the `EventData` variant name, for efficient querying) alongside `data TEXT` (JSON of the variant's typed fields)
- `tasks` — task tree with status tracking
- `task_deps` — task dependency edges
- `agents` — agent lifecycle and token tracking
- `modes` — mode definitions (builtin + generated)
- `recommendations` — UX agent recommendations
- `stats_rollups` — aggregated metrics retained long-term
- `ux_agent_state` — persisted cursor for "since last run" processing

### Migrations

- Schema versioning and migration system
- Initial migration creating all tables and indexes

### Retention

- Raw operational tables are retained for 30 days
- Aggregated stats (`stats_rollups`) are retained indefinitely unless manually cleared
- Retention job evicts raw rows only after rollups are persisted

### Data Access

- Typed Rust structs matching each table
- Insert/query/update functions per entity
- Connection pool or single-connection wrapper (SQLite is single-writer)

## Key Decisions

- SQLite via `rusqlite` or `sqlx` (with SQLite feature)
- Database file location: `.kernel/kernel.db` in project root
- UUIDs stored as `TEXT`
- Event data stored as two columns: `kind TEXT` (the `EventData` enum variant name, derived at write time for query indexing) and `data TEXT` (JSON serialization of the variant's typed fields). Full type safety is enforced in the Rust layer; the `kind` column exists purely for efficient SQL filtering (`WHERE kind = 'ToolFailed'`)
- All timestamps as `DATETIME DEFAULT CURRENT_TIMESTAMP`
- For persisted events, `events.created_at` is the canonical event timestamp
- Retention defaults: 30-day raw data TTL, indefinite stats retention
- Task outcomes are persisted in `tasks.outcome_kind` + `tasks.outcome_data` (JSON)

## Out of Scope

- Event publishing logic (owned by 02 Events)
- Config file parsing (owned by 03 Configuration)
- Business logic for any entity — this sub-system is pure storage
