import { useState, useRef, useEffect, useCallback } from "react";
import type { ChatItem } from "../../hooks/useLlmStream";
import {
  getConversationContext,
  type ContextMessage,
  type ContextBlock,
} from "../../lib/commands";

interface ContextRingProps {
  /** Tokens already consumed (estimated across all turns + current prompt). */
  used: number;
  /** Total context window size for the active model. */
  total: number;
  /** Current conversation items for the summary view. */
  items: ChatItem[];
  /** Session ID to fetch raw context from the backend. */
  sessionId: string;
}

const SIZE = 32;
const STROKE = 2.5;
const RADIUS = (SIZE - STROKE) / 2;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

type Tab = "summary" | "context";

export function ContextRing({ used, total, items, sessionId }: ContextRingProps) {
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState<Tab>("summary");
  const [rawContext, setRawContext] = useState<ContextMessage[] | null>(null);
  const [loading, setLoading] = useState(false);
  const popoverRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);

  const ratio = total > 0 ? Math.min(used / total, 1) : 0;
  const filled = ratio * CIRCUMFERENCE;
  const remaining = CIRCUMFERENCE - filled;

  const arcColor =
    ratio < 0.6
      ? "var(--mode-tint)"
      : ratio < 0.85
        ? "hsl(38 90% 60%)"
        : "hsl(4 80% 60%)";

  const pct = Math.round(ratio * 100);

  // Close on outside click
  useEffect(() => {
    if (!open) return;
    function handleClick(e: MouseEvent) {
      if (
        popoverRef.current &&
        !popoverRef.current.contains(e.target as Node) &&
        buttonRef.current &&
        !buttonRef.current.contains(e.target as Node)
      ) {
        setOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, [open]);

  // Fetch raw context when switching to the context tab
  const fetchContext = useCallback(async () => {
    setLoading(true);
    try {
      const ctx = await getConversationContext(sessionId);
      setRawContext(ctx);
    } catch {
      setRawContext(null);
    } finally {
      setLoading(false);
    }
  }, [sessionId]);

  useEffect(() => {
    if (open && tab === "context") fetchContext();
  }, [open, tab, fetchContext]);

  const summary = summarizeContext(items);

  return (
    <div className="relative">
      <button
        ref={buttonRef}
        onClick={() => setOpen((v) => !v)}
        className="relative flex items-center justify-center shrink-0 cursor-pointer hover:opacity-80 transition-opacity"
        style={{ width: SIZE, height: SIZE }}
        title={`Context: ~${used.toLocaleString()} / ${total.toLocaleString()} tokens (${pct}%)`}
      >
        <svg
          width={SIZE}
          height={SIZE}
          viewBox={`0 0 ${SIZE} ${SIZE}`}
          style={{ transform: "rotate(-90deg)" }}
          aria-hidden="true"
        >
          <circle
            cx={SIZE / 2}
            cy={SIZE / 2}
            r={RADIUS}
            fill="none"
            stroke="var(--color-border-default)"
            strokeWidth={STROKE}
          />
          {ratio > 0 && (
            <circle
              cx={SIZE / 2}
              cy={SIZE / 2}
              r={RADIUS}
              fill="none"
              stroke={arcColor}
              strokeWidth={STROKE}
              strokeLinecap="round"
              strokeDasharray={`${filled} ${remaining}`}
              style={{ transition: "stroke-dasharray 0.5s ease, stroke 0.5s ease" }}
            />
          )}
        </svg>
        <span
          className="absolute text-[7px] font-mono leading-none tabular-nums pointer-events-none"
          style={{
            color: ratio > 0 ? arcColor : "var(--color-text-ghost)",
            transition: "color 0.5s ease",
          }}
        >
          {pct}
        </span>
      </button>

      {open && (
        <div
          ref={popoverRef}
          className="absolute bottom-full right-0 mb-2 w-[420px] max-h-[520px] flex flex-col rounded-xl border border-white/[0.06] bg-surface-1 shadow-xl z-50"
        >
          {/* Header */}
          <div className="shrink-0 border-b border-white/[0.06] px-4 py-3">
            <div className="flex items-center justify-between">
              <span className="text-[12px] font-medium text-text-primary tracking-wide">
                Context Window
              </span>
              <span className="text-[11px] font-mono tabular-nums" style={{ color: arcColor }}>
                {used.toLocaleString()} / {total.toLocaleString()}
              </span>
            </div>
            <div className="mt-2 h-1.5 rounded-full bg-white/[0.04] overflow-hidden">
              <div
                className="h-full rounded-full transition-all duration-500"
                style={{ width: `${pct}%`, backgroundColor: arcColor }}
              />
            </div>
            {/* Tabs */}
            <div className="mt-3 flex gap-1">
              <TabButton active={tab === "summary"} onClick={() => setTab("summary")}>
                Summary
              </TabButton>
              <TabButton active={tab === "context"} onClick={() => setTab("context")}>
                Raw Context
              </TabButton>
            </div>
          </div>

          {/* Body */}
          <div className="flex-1 overflow-y-auto min-h-0">
            {tab === "summary" ? (
              <SummaryTab items={items} summary={summary} />
            ) : (
              <RawContextTab messages={rawContext} loading={loading} onRefresh={fetchContext} />
            )}
          </div>
        </div>
      )}
    </div>
  );
}

// ---- Sub-components ----

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className={`px-2.5 py-1 rounded-md text-[10px] font-medium tracking-wide transition-colors ${
        active
          ? "bg-white/[0.08] text-text-primary"
          : "text-text-ghost hover:text-text-secondary hover:bg-white/[0.04]"
      }`}
    >
      {children}
    </button>
  );
}

function SummaryTab({ items, summary }: { items: ChatItem[]; summary: ReturnType<typeof summarizeContext> }) {
  return (
    <div>
      <div className="px-4 py-3 space-y-2">
        <Row label="User messages" value={summary.userMessages} />
        <Row label="Assistant messages" value={summary.assistantMessages} />
        <Row label="Tool calls" value={summary.toolCalls} />
        <Row label="Tool results" value={summary.toolResults} />
        <Divider />
        <Row label="Total items" value={summary.totalItems} bold />
      </div>

      {items.length > 0 && (
        <div className="border-t border-white/[0.06] px-4 py-3">
          <p className="text-[10px] text-text-ghost uppercase tracking-widest mb-2">Messages</p>
          <div className="space-y-1">
            {items.map((item, i) => (
              <SummaryItem key={i} item={item} />
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function RawContextTab({
  messages,
  loading,
  onRefresh,
}: {
  messages: ContextMessage[] | null;
  loading: boolean;
  onRefresh: () => void;
}) {
  if (loading) {
    return (
      <div className="flex items-center justify-center py-12">
        <span className="text-[11px] text-text-ghost animate-pulse">Loading context...</span>
      </div>
    );
  }

  if (!messages || messages.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center py-12 gap-2">
        <span className="text-[11px] text-text-ghost">No context yet</span>
        <button
          onClick={onRefresh}
          className="text-[10px] text-text-secondary hover:text-text-primary transition-colors"
        >
          Refresh
        </button>
      </div>
    );
  }

  return (
    <div className="divide-y divide-white/[0.04]">
      <div className="px-4 py-2 flex items-center justify-between">
        <span className="text-[10px] text-text-ghost">
          {messages.length} message{messages.length !== 1 ? "s" : ""} in LLM context
        </span>
        <button
          onClick={onRefresh}
          className="text-[10px] text-text-ghost hover:text-text-secondary transition-colors"
        >
          Refresh
        </button>
      </div>
      {messages.map((msg, i) => (
        <RawMessageBlock key={i} index={i} message={msg} />
      ))}
    </div>
  );
}

function RawMessageBlock({ index, message }: { index: number; message: ContextMessage }) {
  const [expanded, setExpanded] = useState(false);

  const blockSummary = message.content.map(summarizeBlock).join(" + ");
  const charCount = message.content.reduce((n, b) => n + blockCharCount(b), 0);
  const tokenEst = Math.ceil(charCount / 4);

  return (
    <div className="px-4 py-2">
      <button
        onClick={() => setExpanded((v) => !v)}
        className="w-full flex items-center gap-2 text-left group cursor-pointer"
      >
        <span className="text-[9px] font-mono text-text-ghost w-4 shrink-0 text-right">
          {index}
        </span>
        <span
          className={`text-[10px] font-mono shrink-0 ${
            message.role === "user" ? "text-blue-400" : "text-green-400"
          }`}
        >
          {message.role}
        </span>
        <span className="text-[10px] text-text-ghost truncate flex-1">{blockSummary}</span>
        <span className="text-[9px] font-mono text-text-ghost tabular-nums shrink-0">
          ~{tokenEst}t
        </span>
        <span className="text-[10px] text-text-ghost group-hover:text-text-secondary transition-colors">
          {expanded ? "▾" : "▸"}
        </span>
      </button>

      {expanded && (
        <div className="mt-2 ml-6 space-y-2">
          {message.content.map((block, bi) => (
            <BlockDetail key={bi} block={block} />
          ))}
        </div>
      )}
    </div>
  );
}

function BlockDetail({ block }: { block: ContextBlock }) {
  const [fullExpanded, setFullExpanded] = useState(false);

  if (block.type === "text") {
    const long = block.text.length > 300;
    const display = long && !fullExpanded ? block.text.slice(0, 300) + "..." : block.text;
    return (
      <div>
        <span className="text-[9px] font-mono text-text-ghost">text</span>
        <pre className="mt-0.5 text-[10px] text-text-secondary whitespace-pre-wrap break-words leading-snug max-h-[200px] overflow-y-auto">
          {display}
        </pre>
        {long && (
          <button
            onClick={() => setFullExpanded((v) => !v)}
            className="text-[9px] text-text-ghost hover:text-text-secondary mt-0.5"
          >
            {fullExpanded ? "Collapse" : `Show all (${block.text.length} chars)`}
          </button>
        )}
      </div>
    );
  }

  if (block.type === "tool_use") {
    return (
      <div>
        <span className="text-[9px] font-mono text-amber-400">tool_use</span>
        <span className="text-[9px] text-text-ghost ml-1">{block.name}</span>
        <pre className="mt-0.5 text-[10px] text-text-ghost whitespace-pre-wrap break-words leading-snug max-h-[120px] overflow-y-auto">
          {JSON.stringify(block.input, null, 2)}
        </pre>
      </div>
    );
  }

  if (block.type === "tool_result") {
    const long = block.content.length > 300;
    const display = long && !fullExpanded ? block.content.slice(0, 300) + "..." : block.content;
    return (
      <div>
        <span className={`text-[9px] font-mono ${block.is_error ? "text-red-400" : "text-teal-400"}`}>
          tool_result
        </span>
        <pre className="mt-0.5 text-[10px] text-text-ghost whitespace-pre-wrap break-words leading-snug max-h-[200px] overflow-y-auto">
          {display}
        </pre>
        {long && (
          <button
            onClick={() => setFullExpanded((v) => !v)}
            className="text-[9px] text-text-ghost hover:text-text-secondary mt-0.5"
          >
            {fullExpanded ? "Collapse" : `Show all (${block.content.length} chars)`}
          </button>
        )}
      </div>
    );
  }

  return null;
}

// ---- Shared small components ----

function Row({ label, value, bold }: { label: string; value: string | number; bold?: boolean }) {
  return (
    <div className="flex items-center justify-between">
      <span className={`text-[11px] ${bold ? "text-text-secondary" : "text-text-ghost"}`}>
        {label}
      </span>
      <span
        className={`text-[11px] font-mono tabular-nums ${bold ? "text-text-primary" : "text-text-secondary"}`}
      >
        {value}
      </span>
    </div>
  );
}

function Divider() {
  return <div className="border-t border-white/[0.04]" />;
}

function SummaryItem({ item }: { item: ChatItem }) {
  if (item.kind === "text") {
    const preview = item.content.slice(0, 80) + (item.content.length > 80 ? "..." : "");
    return (
      <div className="flex gap-2 text-[10px] leading-snug">
        <span
          className={`shrink-0 font-mono ${item.role === "user" ? "text-blue-400" : "text-green-400"}`}
        >
          {item.role === "user" ? "USR" : "AST"}
        </span>
        <span className="text-text-ghost truncate">{preview}</span>
      </div>
    );
  }
  if (item.kind === "tool_call") {
    return (
      <div className="flex gap-2 text-[10px] leading-snug">
        <span className="shrink-0 font-mono text-amber-400">TUL</span>
        <span className="text-text-ghost truncate">{item.name}</span>
      </div>
    );
  }
  if (item.kind === "tool_result") {
    return (
      <div className="flex gap-2 text-[10px] leading-snug">
        <span className={`shrink-0 font-mono ${item.isError ? "text-red-400" : "text-teal-400"}`}>
          {item.isError ? "ERR" : "RES"}
        </span>
        <span className="text-text-ghost truncate">
          {item.content.slice(0, 60) + (item.content.length > 60 ? "..." : "")}
        </span>
      </div>
    );
  }
  if (item.kind === "compaction") {
    return (
      <div className="flex gap-2 text-[10px] leading-snug">
        <span className="shrink-0 font-mono text-amber-500">CMP</span>
        <span className="text-text-ghost truncate">
          {item.beforeMessages} &rarr; {item.afterMessages} messages
        </span>
      </div>
    );
  }
  return null;
}

// ---- Helpers ----

function summarizeBlock(block: ContextBlock): string {
  if (block.type === "text") {
    const preview = block.text.slice(0, 40);
    return preview + (block.text.length > 40 ? "..." : "");
  }
  if (block.type === "tool_use") return `${block.name}()`;
  if (block.type === "tool_result") return block.is_error ? "error" : "result";
  return "?";
}

function blockCharCount(block: ContextBlock): number {
  if (block.type === "text") return block.text.length;
  if (block.type === "tool_use") return JSON.stringify(block.input).length + block.name.length;
  if (block.type === "tool_result") return block.content.length;
  return 0;
}

function summarizeContext(items: ChatItem[]) {
  let userMessages = 0;
  let assistantMessages = 0;
  let toolCalls = 0;
  let toolResults = 0;

  for (const item of items) {
    if (item.kind === "text" && item.role === "user") userMessages++;
    else if (item.kind === "text" && item.role === "assistant") assistantMessages++;
    else if (item.kind === "tool_call") toolCalls++;
    else if (item.kind === "tool_result") toolResults++;
  }

  return { userMessages, assistantMessages, toolCalls, toolResults, totalItems: items.length };
}
