import { useState, useMemo, useEffect, useRef, useCallback } from "react";
import type { Session, Mode, KernelConfig } from "../../lib/types";
import { getModeTint } from "../../lib/modeTint";
import { getConversationContext, getAttachedPlan, type ContextMessage } from "../../lib/commands";
import { SessionBar } from "./SessionBar";
import { ModeSelector } from "./ModeSelector";
import { ModelBadge } from "./ModelBadge";
import { PromptInput } from "./PromptInput";
import { ContextRing } from "./ContextRing";
import { ToolCallBlock, ToolResultBlock } from "./ToolBlock";
import { MarkdownMessage } from "./MarkdownMessage";
import { DiffView } from "./DiffView";
import { PlanViewer } from "./PlanViewer";
import { useLlmStream, type ChatItem } from "../../hooks/useLlmStream";
import { cn } from "../../lib/cn";

interface PromptWindowProps {
  session: Session;
  modes: Mode[];
  config: KernelConfig | null;
  onClose: () => void;
}

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
          content: block.text ?? "",
        });
      } else if (block.type === "tool_use") {
        items.push({
          kind: "tool_call",
          id: block.id ?? "",
          name: block.name ?? "",
          input: block.input ?? {},
        });
      } else if (block.type === "tool_result") {
        const content = Array.isArray(block.content)
          ? block.content.map((c) => c.text ?? "").join("")
          : (block.content ?? "");
        items.push({
          kind: "tool_result",
          id: block.tool_use_id ?? "",
          content,
          isError: block.is_error ?? false,
        });
      }
    }
  }
  return items;
}

function resolveModel(mode: Mode, config: KernelConfig | null): string {
  if (mode.default_model) return mode.default_model;
  return config?.models?.default ?? "unknown";
}

const DEFAULT_CONTEXT_WINDOW = 200_000;
const SCROLL_THRESHOLD = 50; // px from bottom to be considered "following"

export function PromptWindow({ session, modes, config, onClose }: PromptWindowProps) {
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
  const [pinned, setPinned] = useState(false);
  const { items, phase, resolvedMode, error, contextUsage, sessionCost, submit, cancel } = useLlmStream(session.id);
  const activeModel = resolvedMode?.model ?? resolveModel(selectedMode, config);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const scrollContainerRef = useRef<HTMLDivElement>(null);

  // Track whether the user is near the bottom
  const [isFollowing, setIsFollowing] = useState(true);
  // Track whether content was previously scrollable (for edge-case detection)
  const wasScrollableRef = useRef(false);

  // History view toggle
  const [historyView, setHistoryView] = useState<HistoryView>("full");
  const [agentContext, setAgentContext] = useState<ChatItem[] | null>(null);
  const [agentContextLoading, setAgentContextLoading] = useState(false);

  const [attachedPlan, setAttachedPlan] = useState<string | null>(null);
  const [planViewerOpen, setPlanViewerOpen] = useState(false);

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

  // Fetch attached plan on mount and when agent turn completes
  useEffect(() => {
    getAttachedPlan(session.id).then(setAttachedPlan).catch(() => {});
  }, [session.id]);

  useEffect(() => {
    if (phase === "idle") {
      getAttachedPlan(session.id).then(setAttachedPlan).catch(() => {});
    }
  }, [phase, session.id]);

  // When the router resolves a mode, update the selector
  useEffect(() => {
    if (resolvedMode) {
      const found = modes.find((m) => m.name === resolvedMode.mode);
      if (found) setSelectedMode(found);
    }
  }, [resolvedMode, modes]);

  // Update isFollowing based on scroll position
  const handleScroll = useCallback(() => {
    const el = scrollContainerRef.current;
    if (!el) return;
    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    setIsFollowing(distanceFromBottom <= SCROLL_THRESHOLD);
  }, []);

  // Smart auto-scroll: follow if already following, or if content just became scrollable
  useEffect(() => {
    const el = scrollContainerRef.current;
    if (!el) return;

    const isScrollable = el.scrollHeight > el.clientHeight;

    if (isFollowing) {
      messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
    } else if (!wasScrollableRef.current && isScrollable) {
      // Content just crossed the scrollable threshold for the first time — follow it
      messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
      setIsFollowing(true);
    }

    wasScrollableRef.current = isScrollable;
  }, [items, agentContext, historyView, isFollowing]);

  // When switching history views, reset following state and scroll to bottom
  useEffect(() => {
    setIsFollowing(true);
    wasScrollableRef.current = false;
  }, [historyView]);

  const scrollToBottom = useCallback(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
    setIsFollowing(true);
  }, []);

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

  // Pre-pass: build set of reverted tool_use_ids and their reasons
  const revertedMap = useMemo(() => {
    const map = new Map<string, string>();
    for (const item of displayItems) {
      if (item.kind === "file_reverted") {
        map.set(item.toolUseId, item.reason);
      }
    }
    return map;
  }, [displayItems]);

  // Pre-pass: build set of tool_use_ids that have a file_change following them
  const fileChangeIds = useMemo(() => {
    const set = new Set<string>();
    for (const item of displayItems) {
      if (item.kind === "file_change") set.add(item.toolUseId);
    }
    return set;
  }, [displayItems]);

  // Pre-pass: build set of plan_create tool_use_ids to suppress their results
  const planCreateIds = useMemo(() => {
    const set = new Set<string>();
    for (const item of displayItems) {
      if (item.kind === "tool_call" && item.name === "plan_create") set.add(item.id);
    }
    return set;
  }, [displayItems]);

  // Use real API token count when available, fall back to chars/4 estimate
  const usedTokens = useMemo(() => {
    if (contextUsage) return contextUsage.inputTokens;
    const text = items
      .map((item) => {
        if (item.kind === "text") return item.content;
        if (item.kind === "tool_call") return JSON.stringify(item.input);
        if (item.kind === "tool_result") return item.content;
        return "";
      })
      .join(" ");
    return Math.round(text.length / 4);
  }, [contextUsage, items]);

  const contextWindow = useMemo(
    () => contextUsage?.contextWindow ?? DEFAULT_CONTEXT_WINDOW,
    [contextUsage],
  );

  async function handleSubmit() {
    const trimmed = prompt.trim();
    if (!trimmed || busy) return;
    // Switch to full view when submitting so user sees live streaming
    if (historyView === "agent") setHistoryView("full");
    const modeOverride = selectedMode.name === "auto" ? null : selectedMode.name;
    const isPinned = pinned;
    setPrompt("");
    setPinned(false);
    await submit(trimmed, modeOverride, isPinned);
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

      {/* View toggle — full width, content centered. Always shown once a
          session has any history so the user can compare their PoV
          (`conversation`) against what the model sees post-compaction
          (`agent context`). */}
      {items.length > 0 && (
        <div className="relative z-10 flex justify-center pt-3 pb-1 shrink-0">
          <ViewToggle view={historyView} onChange={setHistoryView} />
        </div>
      )}

      {/* Messages — scroll container spans full width, inner content is centered */}
      {hasMessages || showPendingBubble ? (
        <div
          ref={scrollContainerRef}
          onScroll={handleScroll}
          className="flex-1 overflow-y-auto"
        >
          {agentContextLoading && historyView === "agent" ? (
            <div className="flex items-center justify-center h-full">
              <span className="text-[11px] text-text-ghost animate-pulse">Loading agent context...</span>
            </div>
          ) : (
            <div className="flex flex-col gap-4 mx-auto w-full max-w-[540px] px-4 py-6">
              {displayItems.map((item, i) => {
                if (item.kind === "compaction") {
                  return (
                    <CompactionDivider
                      key={i}
                      beforeMessages={item.beforeMessages}
                      afterMessages={item.afterMessages}
                    />
                  );
                }

                if (item.kind === "text" && item.role === "user") {
                  return (
                    <div key={i} className="flex justify-end">
                      <div
                        className={cn(
                          "rounded-xl px-4 py-3 text-[14px] leading-relaxed max-w-[85%]",
                          "bg-surface-1 text-text-primary ml-4",
                          item.pinned && "ring-1 ring-amber-400/40",
                        )}
                      >
                        {item.pinned && (
                          <div className="flex items-center gap-1 mb-1">
                            <svg width="10" height="10" viewBox="0 0 16 16" fill="none" className="text-amber-400">
                              <path
                                d="M9.828 1.172a1 1 0 0 1 1.414 0l3.586 3.586a1 1 0 0 1 0 1.414l-2.293 2.293-.707.707-1.414-1.414-2.828 2.828L7 14l-1-1-3.586-3.586L1.5 8.5l3.414.414 2.828-2.828-1.414-1.414.707-.707z"
                                stroke="currentColor"
                                strokeWidth="1.2"
                                fill="currentColor"
                              />
                            </svg>
                            <span className="text-[10px] text-amber-400/70 font-medium tracking-wide">pinned</span>
                          </div>
                        )}
                        <MarkdownMessage content={item.content} role="user" />
                      </div>
                    </div>
                  );
                }

                if (item.kind === "text" && item.role === "assistant") {
                  return (
                    <div key={i} className="flex justify-start">
                      <div className="rounded-xl px-4 py-3 text-[14px] leading-relaxed text-text-secondary mr-4">
                        <MarkdownMessage content={item.content} role="assistant" />
                      </div>
                    </div>
                  );
                }

                if (item.kind === "tool_call") {
                  return (
                    <ToolCallBlock
                      key={i}
                      name={item.name}
                      input={item.input}
                      onPlanClick={() => setPlanViewerOpen(true)}
                    />
                  );
                }

                if (item.kind === "tool_result") {
                  // Suppress the plain text result when a file_change or plan_create follows
                  if (fileChangeIds.has(item.id)) return null;
                  if (planCreateIds.has(item.id)) return null;
                  return (
                    <ToolResultBlock
                      key={i}
                      content={item.content}
                      isError={item.isError}
                    />
                  );
                }

                if (item.kind === "file_change") {
                  return (
                    <DiffView
                      key={i}
                      path={item.path}
                      status={item.status}
                      hunks={item.hunks}
                      beforeContent={item.beforeContent}
                      afterContent={item.afterContent}
                      sessionId={session.id}
                      toolUseId={item.toolUseId}
                      isReverted={revertedMap.has(item.toolUseId)}
                      revertReason={revertedMap.get(item.toolUseId)}
                    />
                  );
                }

                // file_reverted items are consumed by the revertedMap pre-pass
                if (item.kind === "file_reverted") return null;

                return null;
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
        <div className="flex-1" />
      )}

      {/* Input area — centered, fixed width */}
      <div className="shrink-0 w-full max-w-[540px] mx-auto px-4 pb-4 animate-in">
        {/* Scroll-to-bottom button */}
        <div className="relative">
          {!isFollowing && (
            <div className="absolute -top-10 left-0 right-0 flex justify-center pointer-events-none">
              <button
                onClick={scrollToBottom}
                className="pointer-events-auto flex items-center gap-1.5 rounded-full border border-border-subtle bg-surface-1 px-3 py-1 text-[11px] text-text-secondary shadow-sm hover:bg-surface-2 hover:text-text-primary transition-colors"
              >
                <svg width="10" height="10" viewBox="0 0 10 10" fill="none" className="shrink-0">
                  <path d="M5 1v8M1.5 5.5 5 9l3.5-3.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
                </svg>
                new messages
              </button>
            </div>
          )}
        </div>

        <PromptInput
          value={prompt}
          onChange={setPrompt}
          onSubmit={handleSubmit}
          busy={busy}
          onCancel={cancel}
          pinned={pinned}
          onPinToggle={() => setPinned((p) => !p)}
        />

        {/* Controls row */}
        <div className="mt-3 flex items-center justify-between px-1 animate-in-delayed">
          <div className="flex items-center gap-1">
            <ModeSelector
              modes={modes}
              selected={selectedMode}
              onSelect={setSelectedMode}
            />
            {attachedPlan && (
              <>
                <span className="text-text-ghost mx-1">&middot;</span>
                <PlanBadge filename={attachedPlan} onClick={() => setPlanViewerOpen(true)} />
              </>
            )}
            <span className="text-text-ghost mx-1">&middot;</span>
            <ModelBadge model={activeModel} />
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
            {selectedMode.name !== "auto" && (
              <ContextRing used={usedTokens} total={contextWindow} items={items} sessionId={session.id} sessionCost={sessionCost} costThresholds={config?.costs ?? null} />
            )}
          </div>
        </div>
      </div>

      {/* Status */}
      <div className="shrink-0 pb-3 flex justify-center">
        {error ? (
          <div className="max-w-[480px] rounded-lg border border-red-500/20 bg-red-500/5 px-3 py-1.5">
            <p className="text-[11px] text-red-400 tracking-wide text-center">{error}</p>
          </div>
        ) : (
          <p className="text-[11px] text-text-ghost tracking-wide">
            {phase === "classifying" ? (
              "selecting mode..."
            ) : phase === "generating" ? (
              "thinking..."
            ) : phase === "streaming" ? (
              "streaming"
            ) : (
              "ready"
            )}
          </p>
        )}
      </div>

      {/* Plan viewer overlay */}
      {planViewerOpen && attachedPlan && (
        <PlanViewer
          sessionId={session.id}
          filename={attachedPlan}
          onClose={() => setPlanViewerOpen(false)}
        />
      )}
    </div>
  );
}

// ─── Sub-components ────────────────────────────────────────────────────────────

interface ViewToggleProps {
  view: HistoryView;
  onChange: (v: HistoryView) => void;
}

function ViewToggle({ view, onChange }: ViewToggleProps) {
  return (
    <div className="flex items-center gap-0.5 rounded-full border border-border-subtle bg-surface-1 p-0.5 text-[11px]">
      {(["full", "agent"] as HistoryView[]).map((v) => (
        <button
          key={v}
          onClick={() => onChange(v)}
          className={cn(
            "rounded-full px-3 py-0.5 transition-colors",
            view === v
              ? "bg-surface-2 text-text-primary"
              : "text-text-ghost hover:text-text-secondary",
          )}
        >
          {v === "full" ? "conversation" : "agent context"}
        </button>
      ))}
    </div>
  );
}

function PlanBadge({ filename, onClick }: { filename: string; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      className="flex items-center gap-1 text-[11px] font-mono text-text-ghost tracking-tight truncate max-w-[180px] hover:text-text-secondary transition-colors cursor-pointer"
    >
      <svg width="10" height="10" viewBox="0 0 16 16" fill="none" className="shrink-0">
        <path d="M4 1h5.586a1 1 0 0 1 .707.293l3.414 3.414a1 1 0 0 1 .293.707V14a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V2a1 1 0 0 1 1-1Z" stroke="currentColor" strokeWidth="1.5" />
      </svg>
      {filename}
    </button>
  );
}

interface CompactionDividerProps {
  beforeMessages: number;
  afterMessages: number;
}

function CompactionDivider({ beforeMessages, afterMessages }: CompactionDividerProps) {
  return (
    <div className="flex items-center gap-2 py-1">
      <div className="flex-1 border-t border-dashed border-amber-500/20" />
      <span className="text-[10px] text-amber-500/60 shrink-0">
        context compacted {beforeMessages} &rarr; {afterMessages} messages
      </span>
      <div className="flex-1 border-t border-dashed border-amber-500/20" />
    </div>
  );
}
