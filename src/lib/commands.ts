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

export function submitPrompt(
  sessionId: string,
  prompt: string,
  modeOverride: string | null,
) {
  return invoke<void>("submit_prompt", { sessionId, prompt, modeOverride });
}
