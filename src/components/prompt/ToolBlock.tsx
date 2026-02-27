import { useState } from "react";
import { cn } from "../../lib/cn";

interface ToolCallBlockProps {
  name: string;
  input: Record<string, unknown>;
  onPlanClick?: () => void;
}

export function ToolCallBlock({ name, input, onPlanClick }: ToolCallBlockProps) {
  if (name === "plan_create") {
    const title = String(input.title ?? "Plan");
    return (
      <button
        onClick={onPlanClick}
        className="group flex items-center gap-2.5 rounded-lg border border-border-subtle bg-surface-1 px-3 py-2 text-left transition-colors hover:bg-surface-2 hover:border-border-default cursor-pointer w-fit max-w-full"
      >
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" className="shrink-0 text-accent-dim group-hover:text-accent transition-colors">
          <path d="M4 1h5.586a1 1 0 0 1 .707.293l3.414 3.414a1 1 0 0 1 .293.707V14a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V2a1 1 0 0 1 1-1Z" stroke="currentColor" strokeWidth="1.2" />
          <path d="M5.5 8h5M5.5 10.5h5M5.5 5.5h2" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
        </svg>
        <span className="text-[12px] font-medium text-text-secondary group-hover:text-text-primary transition-colors truncate">
          {title}
        </span>
      </button>
    );
  }

  const summary = summarizeArgs(name, input);
  return (
    <div className="flex items-center gap-2 py-1 text-[12px] font-mono text-text-tertiary">
      <span className="text-text-ghost select-none">▸</span>
      <span className="text-accent-dim">{name}</span>
      {summary && (
        <span className="truncate text-text-ghost">{summary}</span>
      )}
    </div>
  );
}

interface ToolResultBlockProps {
  content: string;
  isError: boolean;
}

export function ToolResultBlock({ content, isError }: ToolResultBlockProps) {
  const [expanded, setExpanded] = useState(false);
  const isLong = content.length > 200;
  const display = expanded || !isLong ? content : content.slice(0, 200) + "…";

  return (
    <div
      className={cn(
        "mb-2 rounded-lg px-3 py-2 text-[11px] font-mono leading-relaxed",
        isError
          ? "border border-red-500/10 bg-red-500/5 text-red-400/80"
          : "border border-border-subtle bg-surface-1 text-text-ghost",
        isLong && "cursor-pointer",
      )}
      onClick={() => isLong && setExpanded((e) => !e)}
    >
      <pre className="whitespace-pre-wrap break-words">{display}</pre>
      {isLong && !expanded && (
        <span className="mt-1 block text-[10px] text-text-ghost/60">
          click to expand
        </span>
      )}
    </div>
  );
}

function summarizeArgs(name: string, input: Record<string, unknown>): string {
  switch (name) {
    case "fs_read":
      return String(input.path ?? "");
    case "fs_write":
      return String(input.path ?? "");
    case "glob":
      return String(input.pattern ?? "");
    case "grep":
      return String(input.pattern ?? "");
    case "shell":
      return String(input.command ?? "");
    case "git":
      return String(input.args ?? "");
    case "plan_create":
      return String(input.title ?? "");
    case "plan_search":
      return String(input.query ?? "");
    case "read_plan":
      return "";
    default:
      return JSON.stringify(input).slice(0, 60);
  }
}
