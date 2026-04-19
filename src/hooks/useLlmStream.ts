import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { submitPrompt, cancelPrompt, getConversationHistory, getSessionCost, getModelContextWindow, eventsSince } from "../lib/commands";
import type { ContextBlock } from "../lib/commands";

export interface DiffHunk {
  header: string;
  lines: Array<{ kind: "context" | "add" | "remove"; content: string }>;
}

export type ChatItem =
  | { kind: "text"; role: "user" | "assistant"; content: string; pinned?: boolean }
  | { kind: "tool_call"; id: string; name: string; input: Record<string, unknown> }
  | { kind: "tool_result"; id: string; content: string; isError: boolean }
  | { kind: "file_change"; toolUseId: string; path: string; status: "created" | "modified"; hunks: DiffHunk[]; bytesWritten: number; beforeContent: string | null; afterContent: string }
  | { kind: "file_reverted"; toolUseId: string; path: string; reason: string }
  | { kind: "compaction"; beforeMessages: number; afterMessages: number }
  | { kind: "interrupted" };

/** Distinct phases so the UI can show what's happening. */
export type Phase = "idle" | "classifying" | "generating" | "streaming";

export interface ModeResolved {
  mode: string;
  model: string;
  confidence: number;
}

interface LlmChunk {
  text: string;
}

interface LlmDone {
  stop_reason: string;
  full_text: string;
}

interface LlmError {
  message: string;
}

interface LlmToolCall {
  id: string;
  name: string;
  input: Record<string, unknown>;
}

interface LlmToolResult {
  id: string;
  content: string;
  is_error: boolean;
}

interface FileChangePayload {
  tool_use_id: string;
  path: string;
  status: "created" | "modified";
  hunks: Array<{
    header: string;
    lines: Array<{ kind: "context" | "add" | "remove"; content: string }>;
  }>;
  bytes_written: number;
  before_content: string | null;
  after_content: string;
}

interface FileRevertedPayload {
  tool_use_id: string;
  path: string;
  reason: string;
}

interface LlmUsageEvent {
  session_id: string;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
}

interface ContextUsageEvent {
  session_id: string;
  input_tokens: number;
  context_window: number;
}

export interface ContextUsage {
  inputTokens: number;
  contextWindow: number;
}

// ---------- history bootstrap ----------

interface HistoryResult {
  items: ChatItem[];
  lastMode: ModeResolved | null;
}

function contextBlocksToChatItems(role: "user" | "assistant", content: ContextBlock[], pinned?: boolean): ChatItem[] {
  const items: ChatItem[] = [];
  for (const block of content) {
    if (block.type === "text") {
      items.push({ kind: "text", role, content: block.text, ...(pinned ? { pinned } : {}) });
    } else if (block.type === "tool_use") {
      items.push({ kind: "tool_call", id: block.id, name: block.name, input: block.input });
    } else if (block.type === "tool_result") {
      items.push({ kind: "tool_result", id: block.tool_use_id, content: block.content, isError: block.is_error });
    }
  }
  return items;
}

async function loadHistoryItems(sessionId: string): Promise<HistoryResult> {
  try {
    const [history, events] = await Promise.all([
      getConversationHistory(sessionId),
      eventsSince(sessionId, "1970-01-01T00:00:00Z"),
    ]);

    // Build file change/revert items from events
    const fileChangeItems: ChatItem[] = [];
    for (const ev of events) {
      if (ev.kind === "FileChange") {
        try {
          const d = JSON.parse(ev.data);
          fileChangeItems.push({
            kind: "file_change",
            toolUseId: d.tool_use_id,
            path: d.path,
            status: d.status,
            hunks: d.hunks ?? [],
            bytesWritten: d.bytes_written ?? 0,
            beforeContent: d.before_content ?? null,
            afterContent: d.after_content ?? "",
          });
        } catch { /* skip malformed */ }
      } else if (ev.kind === "FileRevert") {
        try {
          const d = JSON.parse(ev.data);
          fileChangeItems.push({
            kind: "file_reverted",
            toolUseId: d.tool_use_id,
            path: d.path,
            reason: d.reason ?? "",
          });
        } catch { /* skip malformed */ }
      }
    }

    // Convert conversation history entries
    const items: ChatItem[] = history.entries.flatMap((entry) => {
      if (entry.type === "message" && entry.role && entry.content) {
        const chatItems = contextBlocksToChatItems(entry.role, entry.content, entry.pinned);
        // After each tool_result for fs_write, insert the corresponding file_change
        const enriched: ChatItem[] = [];
        for (const item of chatItems) {
          enriched.push(item);
          if (item.kind === "tool_result") {
            // Find matching file_change by toolUseId
            const fc = fileChangeItems.find(
              (fc) => fc.kind === "file_change" && fc.toolUseId === item.id,
            );
            if (fc) enriched.push(fc);
          }
        }
        return enriched;
      }
      if (entry.type === "interrupted") return [{ kind: "interrupted" as const }];
      if (entry.type === "compaction") {
        return [{
          kind: "compaction" as const,
          beforeMessages: entry.before_messages ?? 0,
          afterMessages: entry.after_messages ?? 0,
        }];
      }
      return [];
    });

    // Append any file_reverted items at the end (they're append-only events)
    for (const item of fileChangeItems) {
      if (item.kind === "file_reverted") items.push(item);
    }

    const lastMode = history.last_mode
      ? { mode: history.last_mode.mode, model: history.last_mode.model, confidence: history.last_mode.confidence }
      : null;
    return { items, lastMode };
  } catch {
    return { items: [], lastMode: null };
  }
}

// ---------- hook ----------

export function useLlmStream(sessionId: string) {
  const [items, setItems] = useState<ChatItem[]>([]);
  const [phase, setPhase] = useState<Phase>("idle");
  const [resolvedMode, setResolvedMode] = useState<ModeResolved | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [contextUsage, setContextUsage] = useState<ContextUsage | null>(null);
  const [sessionCost, setSessionCost] = useState<number | null>(null);
  const activeSessionRef = useRef(sessionId);

  // Bootstrap from DB history on mount / session change
  useEffect(() => {
    activeSessionRef.current = sessionId;
    setItems([]);
    setPhase("idle");
    setResolvedMode(null);
    setError(null);
    setContextUsage(null);
    setSessionCost(null);

    let stale = false;
    loadHistoryItems(sessionId).then(async (result) => {
      if (stale) return;
      setItems(result.items);
      if (result.lastMode) {
        setResolvedMode(result.lastMode);
        // Seed the context ring with the last-known model's window so the
        // total doesn't reset to the hardcoded default after a restart.
        // Input tokens stay at 0 until the next live UsageUpdated event.
        try {
          const window = await getModelContextWindow(result.lastMode.model);
          if (!stale && window != null) {
            setContextUsage((prev) => prev ?? { inputTokens: 0, contextWindow: window });
          }
        } catch { /* leave fallback in place */ }
      }
    });
    getSessionCost(sessionId)
      .then((c) => { if (!stale) setSessionCost(c); })
      .catch(() => { if (!stale) setSessionCost(0); });
    return () => { stale = true; };
  }, [sessionId]);

  // Listen to live streaming events
  useEffect(() => {
    let cancelled = false;
    const unlistens: UnlistenFn[] = [];

    const isActive = () => activeSessionRef.current === sessionId;

    async function setup() {
      const u1 = await listen<ModeResolved>("llm-mode-resolved", (e) => {
        if (!isActive()) return;
        setPhase("generating");
        setResolvedMode(e.payload);
      });
      if (cancelled) { u1(); return; }
      unlistens.push(u1);

      const u2 = await listen<LlmChunk>("llm-chunk", (e) => {
        if (!isActive()) return;
        setPhase("streaming");

        setItems((prev) => {
          const last = prev[prev.length - 1];
          if (last?.kind === "text" && last.role === "assistant") {
            return [
              ...prev.slice(0, -1),
              { ...last, content: last.content + e.payload.text },
            ];
          }
          return [...prev, { kind: "text", role: "assistant", content: e.payload.text }];
        });
      });
      if (cancelled) { u2(); return; }
      unlistens.push(u2);

      const u3 = await listen<LlmDone>("llm-done", (e) => {
        if (!isActive()) return;
        if (e.payload.stop_reason === "cancelled") {
          setItems((prev) => [...prev, { kind: "interrupted" }]);
        }
        setPhase("idle");
      });
      if (cancelled) { u3(); return; }
      unlistens.push(u3);

      const u4 = await listen<LlmError>("llm-error", (e) => {
        if (!isActive()) return;
        setError(e.payload.message);
        setPhase("idle");
      });
      if (cancelled) { u4(); return; }
      unlistens.push(u4);

      const u5 = await listen<LlmToolCall>("llm-tool-call", (e) => {
        if (!isActive()) return;
        const { id, name, input } = e.payload;
        setItems((prev) => [...prev, { kind: "tool_call", id, name, input }]);
      });
      if (cancelled) { u5(); return; }
      unlistens.push(u5);

      const u6 = await listen<LlmToolResult>("llm-tool-result", (e) => {
        if (!isActive()) return;
        const { id, content, is_error } = e.payload;
        setItems((prev) => [...prev, { kind: "tool_result", id, content, isError: is_error }]);
      });
      if (cancelled) { u6(); return; }
      unlistens.push(u6);

      const u_fc = await listen<FileChangePayload>("file-change", (e) => {
        if (!isActive()) return;
        const p = e.payload;
        setItems((prev) => [...prev, {
          kind: "file_change",
          toolUseId: p.tool_use_id,
          path: p.path,
          status: p.status,
          hunks: p.hunks,
          bytesWritten: p.bytes_written,
          beforeContent: p.before_content,
          afterContent: p.after_content,
        }]);
      });
      if (cancelled) { u_fc(); return; }
      unlistens.push(u_fc);

      const u_fr = await listen<FileRevertedPayload>("file-reverted", (e) => {
        if (!isActive()) return;
        const p = e.payload;
        setItems((prev) => [...prev, {
          kind: "file_reverted",
          toolUseId: p.tool_use_id,
          path: p.path,
          reason: p.reason,
        }]);
      });
      if (cancelled) { u_fr(); return; }
      unlistens.push(u_fr);

      const u8 = await listen<LlmUsageEvent>("llm-usage", (e) => {
        if (e.payload.session_id !== sessionId) return;
        setSessionCost((prev) => (prev ?? 0) + e.payload.cost_usd);
      });
      if (cancelled) { u8(); return; }
      unlistens.push(u8);

      const u7 = await listen<ContextUsageEvent>("context-usage", (e) => {
        if (e.payload.session_id === sessionId) {
          setContextUsage({
            inputTokens: e.payload.input_tokens,
            contextWindow: e.payload.context_window,
          });
        }
      });
      if (cancelled) { u7(); return; }
      unlistens.push(u7);
    }

    setup();

    return () => {
      cancelled = true;
      unlistens.forEach((u) => u());
    };
  }, [sessionId]);

  const submit = useCallback(
    async (prompt: string, modeOverride: string | null, pinned: boolean = false) => {
      setError(null);
      setPhase("classifying");
      // Optimistically add the user message immediately
      setItems((prev) => [...prev, { kind: "text", role: "user", content: prompt, ...(pinned ? { pinned } : {}) }]);
      try {
        await submitPrompt(sessionId, prompt, modeOverride, pinned);
      } catch (e) {
        setError(String(e));
        setPhase("idle");
      }
    },
    [sessionId],
  );

  const cancel = useCallback(async () => {
    await cancelPrompt(sessionId);
    // Phase transitions to idle when the backend emits llm-done with stop_reason "cancelled"
  }, [sessionId]);

  return { items, phase, resolvedMode, error, contextUsage, sessionCost, submit, cancel };
}
