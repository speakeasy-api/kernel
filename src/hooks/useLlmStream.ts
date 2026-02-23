import { useCallback, useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { submitPrompt, eventsSince } from "../lib/commands";

export type ChatItem =
  | { kind: "text"; role: "user" | "assistant"; content: string }
  | { kind: "tool_call"; id: string; name: string; input: Record<string, unknown> }
  | { kind: "tool_result"; id: string; content: string; isError: boolean }
  | { kind: "compaction"; beforeMessages: number; afterMessages: number };

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
  session_id: string;
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

// ---------- history bootstrap ----------

interface HistoryResult {
  items: ChatItem[];
  lastMode: ModeResolved | null;
}

async function loadHistoryItems(sessionId: string): Promise<HistoryResult> {
  try {
    const events = await eventsSince(sessionId, "2000-01-01T00:00:00");
    const items: ChatItem[] = [];
    let lastMode: ModeResolved | null = null;

    for (const ev of events) {
      if (ev.kind === "PromptSubmitted") {
        const d = JSON.parse(ev.data) as { prompt?: string };
        if (d.prompt) {
          items.push({ kind: "text", role: "user", content: d.prompt });
        }
      } else if (ev.kind === "PromptClassified") {
        const d = JSON.parse(ev.data) as { mode?: string; model?: string; confidence?: number };
        if (d.mode && d.model) {
          lastMode = { mode: d.mode, model: d.model, confidence: d.confidence ?? 0 };
        }
      } else if (ev.kind === "AssistantText") {
        const d = JSON.parse(ev.data) as { text?: string };
        if (d.text) {
          items.push({ kind: "text", role: "assistant", content: d.text });
        }
      } else if (ev.kind === "ToolCall") {
        const d = JSON.parse(ev.data) as { id?: string; name?: string; input?: Record<string, unknown> };
        if (d.id && d.name) {
          items.push({ kind: "tool_call", id: d.id, name: d.name, input: d.input ?? {} });
        }
      } else if (ev.kind === "ToolResult") {
        const d = JSON.parse(ev.data) as { id?: string; content?: string; is_error?: boolean };
        if (d.id) {
          items.push({ kind: "tool_result", id: d.id, content: d.content ?? "", isError: d.is_error ?? false });
        }
      } else if (ev.kind === "ContextCompacted") {
        const d = JSON.parse(ev.data) as { before_messages?: number; after_messages?: number };
        items.push({
          kind: "compaction",
          beforeMessages: d.before_messages ?? 0,
          afterMessages: d.after_messages ?? 0,
        });
      }
    }

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

  // Bootstrap from DB history on mount / session change
  useEffect(() => {
    setItems([]);
    setPhase("idle");
    setResolvedMode(null);
    setError(null);

    loadHistoryItems(sessionId).then((result) => {
      setItems(result.items);
      if (result.lastMode) {
        setResolvedMode(result.lastMode);
      }
    });
  }, [sessionId]);

  // Listen to live streaming events
  useEffect(() => {
    let cancelled = false;
    const unlistens: UnlistenFn[] = [];

    async function setup() {
      const u1 = await listen<ModeResolved>("llm-mode-resolved", (e) => {
        setPhase("generating");
        setResolvedMode(e.payload);
      });
      if (cancelled) { u1(); return; }
      unlistens.push(u1);

      const u2 = await listen<LlmChunk>("llm-chunk", (e) => {
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

      const u3 = await listen<LlmDone>("llm-done", (_e) => {
        setPhase("idle");
      });
      if (cancelled) { u3(); return; }
      unlistens.push(u3);

      const u4 = await listen<LlmError>("llm-error", (e) => {
        setError(e.payload.message);
        setPhase("idle");
      });
      if (cancelled) { u4(); return; }
      unlistens.push(u4);

      const u5 = await listen<LlmToolCall>("llm-tool-call", (e) => {
        const { id, name, input } = e.payload;
        setItems((prev) => [...prev, { kind: "tool_call", id, name, input }]);
      });
      if (cancelled) { u5(); return; }
      unlistens.push(u5);

      const u6 = await listen<LlmToolResult>("llm-tool-result", (e) => {
        const { id, content, is_error } = e.payload;
        setItems((prev) => [...prev, { kind: "tool_result", id, content, isError: is_error }]);
      });
      if (cancelled) { u6(); return; }
      unlistens.push(u6);
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

  return { items, phase, resolvedMode, error, submit };
}
