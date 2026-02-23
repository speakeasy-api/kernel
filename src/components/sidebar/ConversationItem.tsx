import { cn } from "../../lib/cn";
import type { SessionSummary } from "../../hooks/useSessionSummaries";

interface ConversationItemProps {
  summary: SessionSummary;
  isActive: boolean;
  onClick: () => void;
}

function timeAgo(dateStr: string): string {
  const date = new Date(dateStr.endsWith("Z") ? dateStr : dateStr + "Z");
  const now = Date.now();
  const diff = Math.floor((now - date.getTime()) / 1000);

  if (diff < 60) return "just now";
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  if (diff < 86400 * 7) return `${Math.floor(diff / 86400)}d ago`;
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

export function ConversationItem({
  summary,
  isActive,
  onClick,
}: ConversationItemProps) {
  const label = summary.title ?? "New conversation";
  const ago = timeAgo(summary.session.created_at);

  return (
    <button
      onClick={onClick}
      title={label}
      className={cn(
        "group w-full flex items-start gap-2 rounded-md px-2 py-1.5",
        "text-left transition-all duration-100 cursor-pointer",
        isActive
          ? "bg-surface-3 text-text-primary"
          : "text-text-secondary hover:bg-surface-2 hover:text-text-primary active:bg-surface-3",
      )}
    >
      {/* left accent bar */}
      <div
        className={cn(
          "mt-[3px] h-3.5 w-0.5 shrink-0 rounded-full transition-colors duration-100",
          isActive ? "bg-accent" : "bg-transparent group-hover:bg-border-strong",
        )}
      />
      <div className="min-w-0 flex-1">
        <p className="truncate text-[12px] font-medium leading-snug">
          {label}
        </p>
        <p className="mt-0.5 text-[10px] font-mono text-text-ghost tabular-nums">
          {ago}
        </p>
      </div>
    </button>
  );
}
