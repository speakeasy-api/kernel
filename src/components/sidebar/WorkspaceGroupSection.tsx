import { useState } from "react";
import { cn } from "../../lib/cn";
import type { WorkspaceGroup } from "../../hooks/useSessionSummaries";
import type { Session } from "../../lib/types";
import { shortenPathWithHome } from "../../lib/paths";
import { useHomeDir } from "../../hooks/useHomeDir";
import { ConversationItem } from "./ConversationItem";

interface WorkspaceGroupProps {
  group: WorkspaceGroup;
  activeSessionId: string | null;
  defaultOpen?: boolean;
  onSelectSession: (session: Session) => void;
  onNewConversation: (projectPath: string) => void;
}

export function WorkspaceGroupSection({
  group,
  activeSessionId,
  defaultOpen = false,
  onSelectSession,
  onNewConversation,
}: WorkspaceGroupProps) {
  const [open, setOpen] = useState(defaultOpen);
  const home = useHomeDir();

  return (
    <div className="select-none">
      {/* Header / toggle */}
      <div className="group flex items-center rounded-md transition-colors duration-100 hover:bg-surface-2">
        <button
          onClick={() => setOpen((v) => !v)}
          className={cn(
            "flex flex-1 min-w-0 items-center gap-2 px-2 py-1.5 cursor-pointer",
          )}
        >
          {/* chevron */}
          <svg
            className={cn(
              "h-3 w-3 shrink-0 text-text-ghost transition-transform duration-150",
              open && "rotate-90",
            )}
            viewBox="0 0 12 12"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M4 2l4 4-4 4" />
          </svg>

          {/* folder icon */}
          <svg
            className="h-3.5 w-3.5 shrink-0 text-text-tertiary group-hover:text-text-secondary transition-colors duration-100"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M2 4.5V12a1 1 0 001 1h10a1 1 0 001-1V6a1 1 0 00-1-1H8L6.5 3.5H3A1 1 0 002 4.5z" />
          </svg>

          <div className="min-w-0 flex-1 text-left">
            <p className="truncate text-[12px] font-semibold text-text-primary leading-snug">
              {group.projectName}
            </p>
            <p className="truncate text-[10px] font-mono text-text-ghost leading-snug">
              {home ? shortenPathWithHome(group.projectPath, home) : group.projectPath}
            </p>
          </div>

          <span className="shrink-0 text-[10px] font-mono text-text-ghost tabular-nums">
            {group.sessions.length}
          </span>
        </button>

        {/* New conversation button */}
        <button
          onClick={(e) => {
            e.stopPropagation();
            onNewConversation(group.projectPath);
          }}
          className={cn(
            "shrink-0 flex items-center justify-center w-6 h-6 rounded-md mr-1 cursor-pointer",
            "text-text-ghost opacity-0 group-hover:opacity-100 hover:!opacity-100 hover:bg-surface-2 hover:text-text-secondary",
            "transition-all duration-100",
          )}
          style={{ opacity: undefined }}
          title="New conversation"
        >
          <svg
            className="h-3 w-3"
            viewBox="0 0 12 12"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
          >
            <path d="M6 2v8M2 6h8" />
          </svg>
        </button>
      </div>

      {/* Conversation list */}
      {open && (
        <div className="ml-4 mt-0.5 flex flex-col gap-0.5 border-l border-border-subtle pl-2">
          {group.sessions.map((summary) => (
            <ConversationItem
              key={summary.session.id}
              summary={summary}
              isActive={activeSessionId === summary.session.id}
              onClick={() => onSelectSession(summary.session)}
            />
          ))}
        </div>
      )}
    </div>
  );
}
