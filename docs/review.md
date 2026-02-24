Code Review
This is a substantial Tauri-based LLM desktop app ("Kernel") with a Rust backend and React/TypeScript frontend. I've reviewed the entire codebase. Here are my findings, prioritized:

Blocking Issues

- SQLite database files committed to git
  src-tauri/.kernel/kernel.db, kernel.db-shm, kernel.db-wal

More importantly: configuration should live in ~/.kernel/ instead of in each workspace.

High-Priority Issues

- CancellationFlags leaks entries on error paths
  src-tauri/src/prompt_router/commands.rs:47-65

CancellationFlags::create (line 47) inserts into the map, and remove (line 64) cleans up. However, in submit_prompt (line 401+), if an early error occurs after create but before the main loop runs, remove is never called. The HashMap grows unboundedly over the lifetime of the application.

Fix: Use a scope guard or ensure remove is called in all exit paths (e.g., wrap the main body in a block that calls remove in a finally-equivalent pattern).

- Race condition: concurrent submit_prompt on same session
  src-tauri/src/prompt_router/commands.rs:401-410

There's no guard preventing two concurrent submit_prompt calls for the same session_id. CancellationFlags::create silently replaces the previous flag, so the old loop would lose its cancellation handle. The ActiveSessions mutex (line 30) only tracks the cancellation flag, not mutual exclusion of the agent loop.

Keep in mind: we support multiple simultaneous agentic loops at once.

- expect("invalid API key") will panic the entire app
  src-tauri/src/anthropic/client.rs:125

HeaderValue::from_str(&self.api_key).expect("invalid API key"),
If the API key contains invalid header characters, this panics the Tauri backend. Use .map_err() instead and return a proper error.

Medium-Priority Issues

- Hardcoded macOS path prefix /Users/ in three UI components
  src/components/prompt/SessionBar.tsx:13, src/components/sidebar/WorkspaceGroupSection.tsx:16, src/components/workspace/ProjectCard.tsx:12

shortenPath assumes macOS with /Users/. This will show full paths on Linux (/home/) and Windows (C:\Users\). Use a Tauri API to get the home directory, or at least handle multiple platform prefixes.

- useLlmStream doesn't clean up listeners on rapid session switches
  src/hooks/useLlmStream.ts:88-205

The useEffect at line 88 sets up event listeners and returns an unlisten cleanup. But items state persists across the effect re-run — only cleared by a new submit call. If the user switches sessions rapidly, stale listeners from the previous render cycle could interleave events before cleanup runs. The sessionId dependency is correct, but the items state should be reset when sessionId changes.

- No error boundary in the React tree
  src/main.tsx

If any component throws during render, the entire app goes white. Add a React error boundary around <App />.

- useConfig silently swallows errors
  src/hooks/useConfig.ts:10-13

The .catch(() => setConfig(null)) silently discards config parse errors. Consider logging or surfacing config errors to the user — a typo in kernel.toml will silently fall back to defaults with no indication.

- ConversationItem.timeAgo timezone handling
  src/components/sidebar/ConversationItem.tsx:11

const date = new Date(dateStr.endsWith("Z") ? dateStr : dateStr + "Z");
This assumes all non-Z-suffixed timestamps are UTC, which is true for SQLite's datetime('now') but fragile. If the backend ever changes timestamp format, this silently breaks.

- ModelBadge.abbreviateModel doesn't handle new model naming patterns
  src/components/prompt/ModelBadge.tsx:6-10

The regex claude-(\w+)-(\d+)-(\d+) handles names like claude-sonnet-4-6 but would fail for models with different naming conventions (e.g., claude-opus-4-0520). Minor, but could produce ugly display names.

Let's just display model names as is. Let's also add a copy button next to the model name display under the prompt textarea.

- DB pool created without connection limit
  src-tauri/src/db/mod.rs:15-25

The open_project_pool function creates a pool but relies on sqlx defaults. For a desktop app this is probably fine, but consider setting an explicit max connections to avoid resource issues if multiple projects are opened.
