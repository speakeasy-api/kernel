import { useState } from "react";
import { cn } from "../../lib/cn";

interface ToolCallBlockProps {
  name: string;
  input: Record<string, unknown>;
}

export function ToolCallBlock({ name, input }: ToolCallBlockProps) {
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
    default:
      return JSON.stringify(input).slice(0, 60);
  }
}
