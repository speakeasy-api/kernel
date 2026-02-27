import { invoke } from "@tauri-apps/api/core";
import type { Session, RawEvent, DbMode, Mode, KernelConfig } from "./types";

// -- Sessions --

export function createSession(projectPath: string) {
  return invoke<Session>("create_session", { projectPath });
}

export function getSession(id: string) {
  return invoke<Session | null>("get_session", { id });
}

export function listSessions() {
  return invoke<Session[]>("list_sessions");
}

export function deleteSession(id: string) {
  return invoke<void>("delete_session", { id });
}

// -- Events --

export function insertEvent(
  sessionId: string,
  agentId: string | null,
  kind: string,
  data: string,
) {
  return invoke<RawEvent>("insert_event", { sessionId, agentId, kind, data });
}

export function eventsSince(sessionId: string, since: string) {
  return invoke<RawEvent[]>("events_since", { sessionId, since });
}

// -- DB Modes --

export function listDbModes() {
  return invoke<DbMode[]>("list_db_modes");
}

export function getDbMode(name: string) {
  return invoke<DbMode | null>("get_db_mode", { name });
}

// -- Config --

export function loadProjectConfig(projectRoot: string) {
  return invoke<KernelConfig>("load_project_config", { projectRoot });
}

// -- Builtin Modes --

export function getBuiltinModes() {
  return invoke<Mode[]>("get_builtin_modes");
}

// -- Prompt Router --

/** The raw conversation context the LLM sees (for debugging compaction). */
export interface ContextMessage {
  role: "user" | "assistant";
  content: ContextBlock[];
}

export type ContextBlock =
  | { type: "text"; text: string }
  | { type: "tool_use"; id: string; name: string; input: Record<string, unknown> }
  | { type: "tool_result"; tool_use_id: string; content: string; is_error: boolean };

export function getConversationContext(sessionId: string) {
  return invoke<ContextMessage[]>("get_conversation_context", { sessionId });
}

// -- Conversation History --

export interface HistoryEntry {
  type: "message" | "interrupted" | "compaction";
  // message fields (when type === "message"):
  role?: "user" | "assistant";
  content?: ContextBlock[];
  // compaction fields (when type === "compaction"):
  before_messages?: number;
  after_messages?: number;
}

export interface ConversationHistory {
  entries: HistoryEntry[];
  last_mode: { mode: string; model: string; confidence: number } | null;
}

export function getConversationHistory(sessionId: string) {
  return invoke<ConversationHistory>("get_conversation_history", { sessionId });
}

export function submitPrompt(
  sessionId: string,
  prompt: string,
  modeOverride: string | null,
) {
  return invoke<void>("submit_prompt", { sessionId, prompt, modeOverride });
}

export function cancelPrompt(sessionId: string) {
  return invoke<void>("cancel_prompt", { sessionId });
}

// -- Session Cost --

export function getSessionCost(sessionId: string): Promise<number> {
  return invoke<number>("get_session_cost", { sessionId });
}

// -- Attached Plan --

export function getAttachedPlan(sessionId: string): Promise<string | null> {
  return invoke<string | null>("get_attached_plan", { sessionId });
}

// -- File Revert --

export type RevertResult =
  | { status: "success" }
  | { status: "conflict"; expected_hash: string; actual_hash: string }
  | { status: "not_found" }
  | { status: "error"; message: string };

export function revertFile(
  sessionId: string,
  toolUseId: string,
  path: string,
  beforeContent: string | null,
  afterContent: string,
  reason: string,
  force?: boolean,
) {
  return invoke<RevertResult>("revert_file", {
    sessionId,
    toolUseId,
    path,
    beforeContent,
    afterContent,
    reason,
    force: force ?? false,
  });
}
