import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { submitPrompt, cancelPrompt, getConversationHistory } from "../lib/commands";
import type { ContextBlock } from "../lib/commands";

export type ChatItem =
  | { kind: "text"; role: "user" | "assistant"; content: string }
  | { kind: "tool_call"; id: string; name: string; input: Record<string, unknown> }
  | { kind: "tool_result"; id: string; content: string; isError: boolean }
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

function contextBlocksToChatItems(role: "user" | "assistant", content: ContextBlock[]): ChatItem[] {
  const items: ChatItem[] = [];
  for (const block of content) {
    if (block.type === "text") {
      items.push({ kind: "text", role, content: block.text });
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
    const history = await getConversationHistory(sessionId);
    const items: ChatItem[] = history.entries.flatMap((entry) => {
      if (entry.type === "message" && entry.role && entry.content) {
        return contextBlocksToChatItems(entry.role, entry.content);
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
  const activeSessionRef = useRef(sessionId);

  // Bootstrap from DB history on mount / session change
  useEffect(() => {
    activeSessionRef.current = sessionId;
    setItems([]);
    setPhase("idle");
    setResolvedMode(null);
    setError(null);
    setContextUsage(null);

    let stale = false;
    loadHistoryItems(sessionId).then((result) => {
      if (stale) return;
      setItems(result.items);
      if (result.lastMode) {
        setResolvedMode(result.lastMode);
      }
    });
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
    async (prompt: string, modeOverride: string | null) => {
      setError(null);
      setPhase("classifying");
      // Optimistically add the user message immediately
      setItems((prev) => [...prev, { kind: "text", role: "user", content: prompt }]);
      try {
        await submitPrompt(sessionId, prompt, modeOverride);
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

  return { items, phase, resolvedMode, error, contextUsage, submit, cancel };
}
