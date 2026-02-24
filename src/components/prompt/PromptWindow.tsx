import { useState, useMemo, useEffect, useRef, useCallback } from "react";
import type { Session, Mode, KernelConfig } from "../../lib/types";
import { getModeTint } from "../../lib/modeTint";
import { getConversationContext, type ContextMessage } from "../../lib/commands";
import { SessionBar } from "./SessionBar";
import { ModeSelector } from "./ModeSelector";
import { ModelBadge } from "./ModelBadge";
import { PromptInput } from "./PromptInput";
import { ContextRing } from "./ContextRing";
import { ToolCallBlock, ToolResultBlock } from "./ToolBlock";
import { MarkdownMessage } from "./MarkdownMessage";
import { useLlmStream, type ChatItem } from "../../hooks/useLlmStream";
import { cn } from "../../lib/cn";

interface PromptWindowProps {
  session: Session;
  modes: Mode[];
  config: KernelConfig | null;
  onClose: () => void;
}

function resolveModel(mode: Mode, config: KernelConfig | null): string {
  return mode.default_model ?? config?.models.default ?? "claude-sonnet-4-6";
}

/** Conservative fallback before the backend reports real context window. */
const DEFAULT_CONTEXT_WINDOW = 128_000;

type HistoryView = "full" | "agent";

/** Convert raw ContextMessages (what the LLM sees) into ChatItems for display. */
function contextToChatItems(messages: ContextMessage[]): ChatItem[] {
  const items: ChatItem[] = [];
  for (const msg of messages) {
    for (const block of msg.content) {
      if (block.type === "text") {
        items.push({
          kind: "text",
          role: msg.role as "user" | "assistant",
          content: block.text,
        });
      } else if (block.type === "tool_use") {
        items.push({
          kind: "tool_call",
          id: block.id,
          name: block.name,
          input: block.input,
        });
      } else if (block.type === "tool_result") {
        items.push({
          kind: "tool_result",
          id: block.tool_use_id,
          content: block.content,
          isError: block.is_error,
        });
      }
    }
  }
  return items;
}

export function PromptWindow({
  session,
  modes,
  config,
  onClose,
}: PromptWindowProps) {
  const [selectedMode, setSelectedMode] = useState<Mode>(
    () => ({
      name: "auto",
      description: "Let Kernel choose",
      system_prompt: "",
      default_model: null,
      allowed_tools: [],
      created_by: "builtin" as const,
      version: 1,
    }),
  );
  const [prompt, setPrompt] = useState("");
  const { items, phase, resolvedMode, error, contextUsage, submit, cancel } = useLlmStream(session.id);
  const messagesEndRef = useRef<HTMLDivElement>(null);

  // History view toggle
  const [historyView, setHistoryView] = useState<HistoryView>("full");
  const [agentContext, setAgentContext] = useState<ChatItem[] | null>(null);
  const [agentContextLoading, setAgentContextLoading] = useState(false);

  const busy = phase !== "idle";

  // Global Escape key listener (works even when textarea isn't focused)
  useEffect(() => {
    if (!busy) return;
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        cancel();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [busy, cancel]);

  // When the router resolves a mode, update the selector
  useEffect(() => {
    if (resolvedMode) {
      const found = modes.find((m) => m.name === resolvedMode.mode);
      if (found) setSelectedMode(found);
    }
  }, [resolvedMode, modes]);

  // Auto-scroll to bottom on new items
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [items, agentContext, historyView]);

  // Fetch agent context when switching to agent view
  const fetchAgentContext = useCallback(async () => {
    setAgentContextLoading(true);
    try {
      const ctx = await getConversationContext(session.id);
      setAgentContext(contextToChatItems(ctx));
    } catch {
      setAgentContext(null);
    } finally {
      setAgentContextLoading(false);
    }
  }, [session.id]);

  useEffect(() => {
    if (historyView === "agent") fetchAgentContext();
  }, [historyView, fetchAgentContext]);

  // Refresh agent context when streaming completes
  useEffect(() => {
    if (phase === "idle" && historyView === "agent") {
      fetchAgentContext();
    }
  }, [phase, historyView, fetchAgentContext]);

  const tint = useMemo(
    () => getModeTint(selectedMode.name),
    [selectedMode.name],
  );

  const displayItems = historyView === "agent" && agentContext ? agentContext : items;

  // Use real API token count when available, fall back to chars/4 estimate
  const usedTokens = useMemo(() => {
    if (contextUsage) return contextUsage.inputTokens;
    const historyChars = items.reduce<number>((acc, item) => {
      if (item.kind === "text") return acc + item.content.length;
      if (item.kind === "tool_result") return acc + item.content.length;
      if (item.kind === "tool_call") return acc + JSON.stringify(item.input).length;
      return acc;
    }, 0);
    return Math.ceil((historyChars + prompt.length) / 4);
  }, [contextUsage, items, prompt]);

  const contextWindow = useMemo(
    () => contextUsage?.contextWindow ?? DEFAULT_CONTEXT_WINDOW,
    [contextUsage],
  );

  async function handleSubmit() {
    const trimmed = prompt.trim();
    if (!trimmed || busy) return;
    // Switch back to full view when submitting so user sees live streaming
    if (historyView === "agent") setHistoryView("full");
    const modeOverride = selectedMode.name === "auto" ? null : selectedMode.name;
    setPrompt("");
    await submit(trimmed, modeOverride);
  }

  const hasMessages = displayItems.length > 0;

  // Show a ghost assistant bubble with cursor while classifying/generating (before first chunk)
  const showPendingBubble = phase === "classifying" || phase === "generating";

  return (
    <div
      className="flex h-full flex-col bg-surface-0 transition-colors duration-500"
      style={tint.vars}
    >
      <SessionBar session={session} onClose={onClose} />

      <div className="relative flex flex-1 flex-col px-8 pb-6 overflow-hidden">
        {/* Mode-tinted ambient glow */}
        <div
          className="pointer-events-none absolute left-1/2 top-[42%] -translate-x-1/2 -translate-y-1/2 h-[260px] w-[480px] rounded-full blur-[120px] transition-all duration-700"
          style={{ background: `radial-gradient(ellipse, var(--mode-tint-glow) 0%, transparent 70%)` }}
        />

        {/* History view toggle */}
        {items.length > 0 && (
          <div className="relative z-10 flex justify-center pt-3 pb-1">
            <ViewToggle view={historyView} onChange={setHistoryView} />
          </div>
        )}

        {/* Messages area */}
        {hasMessages || showPendingBubble ? (
          <div className="relative flex-1 overflow-y-auto py-6">
            {agentContextLoading && historyView === "agent" ? (
              <div className="flex items-center justify-center h-full">
                <span className="text-[11px] text-text-ghost animate-pulse">Loading agent context...</span>
              </div>
            ) : (
              <div className="mx-auto max-w-[640px] space-y-4">
                {historyView === "agent" && (
                  <div className="flex items-center gap-2 px-3 py-2 rounded-lg bg-white/[0.03] border border-white/[0.06] mb-2">
                    <span className="text-[10px] text-text-ghost">
                      Showing what the agent sees after compaction. Some messages may be summarized or removed.
                    </span>
                  </div>
                )}

                {displayItems.map((item, i) => {
                  if (item.kind === "compaction") {
                    return (
                      <CompactionMarker
                        key={i}
                        beforeMessages={item.beforeMessages}
                        afterMessages={item.afterMessages}
                      />
                    );
                  }
                  if (item.kind === "interrupted") {
                    return <InterruptedMarker key={i} />;
                  }
                  if (item.kind === "tool_call") {
                    return <ToolCallBlock key={i} name={item.name} input={item.input} />;
                  }
                  if (item.kind === "tool_result") {
                    return <ToolResultBlock key={i} content={item.content} isError={item.isError} />;
                  }
                  return (
                    <div
                      key={i}
                      className={cn(
                        "rounded-xl px-4 py-3 text-[14px] leading-relaxed",
                        item.role === "user"
                          ? "bg-surface-2 text-text-primary ml-12"
                          : "text-text-secondary mr-4",
                      )}
                    >
                      <div className="markdown-message wrap-break-word">
                        <MarkdownMessage content={item.content} role={item.role} />
                        {item.role === "assistant" &&
                          phase === "streaming" &&
                          historyView === "full" &&
                          i === displayItems.length - 1 && (
                            <span
                              className="inline-block w-1.5 h-4 rounded-sm ml-0.5 -mb-0.5 animate-caret"
                              style={{ backgroundColor: "var(--mode-tint)" }}
                            />
                          )}
                      </div>
                    </div>
                  );
                })}

                {/* Shows a blinking cursor before any text arrives */}
                {showPendingBubble && historyView === "full" && (
                  <div className="rounded-xl px-4 py-3 text-[14px] leading-relaxed text-text-secondary mr-4">
                    <span className="inline-block w-1.5 h-4 rounded-sm animate-caret" style={{ backgroundColor: "var(--mode-tint)" }} />
                  </div>
                )}

                <div ref={messagesEndRef} />
              </div>
            )}
          </div>
        ) : (
          <div className="flex flex-1 items-center justify-center" />
        )}

        {/* Input area */}
        <div className="relative w-full max-w-[540px] mx-auto animate-in">
          <PromptInput
            value={prompt}
            onChange={setPrompt}
            onSubmit={handleSubmit}
            busy={busy}
            onCancel={cancel}
          />

          {/* Controls row */}
          <div className="mt-3 flex items-center justify-between px-1 animate-in-delayed">
            <div className="flex items-center gap-1">
              <ModeSelector
                modes={modes}
                selected={selectedMode}
                onSelect={setSelectedMode}
              />
              <span className="text-text-ghost mx-1">&middot;</span>
              <ModelBadge model={resolveModel(selectedMode, config)} />
            </div>

            <div className="flex items-center gap-2">
              {busy && (
                <span className="text-[11px] font-mono tracking-tight animate-pulse" style={{ color: "var(--mode-tint-dim)" }}>
                  {phase === "classifying"
                    ? "routing..."
                    : phase === "generating"
                      ? "thinking..."
                      : "streaming..."}
                </span>
              )}
              <ContextRing used={usedTokens} total={contextWindow} items={items} sessionId={session.id} />
            </div>
          </div>
        </div>

        {/* Status */}
        <div className="mt-3 flex justify-center">
          {error ? (
            <div className="max-w-[480px] rounded-lg border border-red-500/20 bg-red-500/5 px-3 py-1.5">
              <p className="text-[11px] text-red-400 tracking-wide text-center">{error}</p>
            </div>
          ) : (
            <p className="text-[11px] text-text-ghost tracking-wide">
              {phase === "classifying" ? (
                "selecting mode..."
              ) : phase === "generating" ? (
                resolvedMode
                  ? `${resolvedMode.mode} mode`
                  : "waiting for model..."
              ) : phase === "streaming" ? (
                resolvedMode
                  ? `${resolvedMode.mode}`
                  : "streaming"
              ) : (
                "ready"
              )}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

// ---- Sub-components ----

function ViewToggle({
  view,
  onChange,
}: {
  view: HistoryView;
  onChange: (v: HistoryView) => void;
}) {
  return (
    <div className="inline-flex rounded-lg border border-white/[0.06] bg-surface-1 p-0.5">
      <button
        onClick={() => onChange("full")}
        className={cn(
          "px-2.5 py-1 rounded-md text-[10px] font-medium tracking-wide transition-colors",
          view === "full"
            ? "bg-white/[0.08] text-text-primary"
            : "text-text-ghost hover:text-text-secondary",
        )}
      >
        Full History
      </button>
      <button
        onClick={() => onChange("agent")}
        className={cn(
          "px-2.5 py-1 rounded-md text-[10px] font-medium tracking-wide transition-colors",
          view === "agent"
            ? "bg-white/[0.08] text-text-primary"
            : "text-text-ghost hover:text-text-secondary",
        )}
      >
        Agent View
      </button>
    </div>
  );
}

function InterruptedMarker() {
  return (
    <div className="flex items-center gap-3 py-2">
      <div className="flex-1 border-t border-dashed border-text-ghost/20" />
      <span className="text-[10px] font-mono text-text-ghost/60 shrink-0">
        interrupted by user
      </span>
      <div className="flex-1 border-t border-dashed border-text-ghost/20" />
    </div>
  );
}

function CompactionMarker({
  beforeMessages,
  afterMessages,
}: {
  beforeMessages: number;
  afterMessages: number;
}) {
  return (
    <div className="flex items-center gap-3 py-2">
      <div className="flex-1 border-t border-dashed border-amber-500/20" />
      <span className="text-[10px] font-mono text-amber-500/60 shrink-0">
        context compacted {beforeMessages} &rarr; {afterMessages} messages
      </span>
      <div className="flex-1 border-t border-dashed border-amber-500/20" />
    </div>
  );
}
