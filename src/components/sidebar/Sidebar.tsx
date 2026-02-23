import { useEffect, useRef } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { cn } from "../../lib/cn";
import type { Session } from "../../lib/types";
import { createSession } from "../../lib/commands";
import { useSessionSummaries } from "../../hooks/useSessionSummaries";
import { WorkspaceGroupSection } from "./WorkspaceGroupSection";
import { Spinner } from "../ui/Spinner";

interface SidebarProps {
  activeSessionId: string | null;
  onSelectSession: (session: Session) => void;
  onNewSession: (session: Session) => void;
  /** Called whenever the sidebar refreshes so App can react if needed. */
  refreshKey?: number;
}

export function Sidebar({
  activeSessionId,
  onSelectSession,
  onNewSession,
  refreshKey,
}: SidebarProps) {
  const { groups, loading, refresh } = useSessionSummaries();
  const prevRefreshKey = useRef(refreshKey);

  // Re-fetch whenever the parent signals a refresh (e.g. new session created)
  useEffect(() => {
    if (refreshKey !== prevRefreshKey.current) {
      prevRefreshKey.current = refreshKey;
      refresh();
    }
  }, [refreshKey, refresh]);

  async function handleOpenFolder() {
    const selected = await open({ directory: true, multiple: false });
    if (!selected) return;
    const path = typeof selected === "string" ? selected : selected;
    const session = await createSession(path as string);
    await refresh();
    onNewSession(session);
  }

  async function handleNewConversation(projectPath: string) {
    const session = await createSession(projectPath);
    onNewSession(session);
  }

  return (
    <aside
      className="flex h-full w-56 shrink-0 flex-col border-r border-border-subtle bg-surface-0"
      style={{ WebkitAppRegion: "drag" } as React.CSSProperties}
    >
      {/* Title bar area — traffic lights live here on macOS */}
      <div className="flex h-10 shrink-0 items-center px-3">
        <span className="text-[11px] font-semibold uppercase tracking-widest text-text-ghost">
          Workspaces
        </span>
      </div>

      {/* Scrollable workspace list */}
      <div
        className="flex-1 overflow-y-auto px-2 pb-4"
        style={{ WebkitAppRegion: "no-drag" } as React.CSSProperties}
      >
        {loading ? (
          <div className="flex items-center justify-center py-8">
            <Spinner />
          </div>
        ) : groups.length === 0 ? (
          <p className="px-2 py-6 text-center text-[12px] text-text-ghost">
            No workspaces yet.
            <br />
            Open a folder to begin.
          </p>
        ) : (
          <div className="flex flex-col gap-1">
            {groups.map((group, i) => (
              <WorkspaceGroupSection
                key={group.projectPath}
                group={group}
                activeSessionId={activeSessionId}
                defaultOpen={i === 0}
                onSelectSession={onSelectSession}
                onNewConversation={handleNewConversation}
              />
            ))}
          </div>
        )}
      </div>

      {/* Footer — Open folder button */}
      <div
        className="shrink-0 border-t border-border-subtle p-2"
        style={{ WebkitAppRegion: "no-drag" } as React.CSSProperties}
      >
        <button
          onClick={handleOpenFolder}
          className={cn(
            "group flex w-full items-center gap-2 rounded-lg border border-dashed border-border-default",
            "px-3 py-2 text-[12px] font-medium text-text-ghost",
            "transition-all duration-150 hover:border-border-strong hover:bg-surface-1/50 hover:text-text-secondary",
            "active:scale-[0.99] cursor-pointer",
          )}
        >
          <svg
            className="h-3.5 w-3.5 shrink-0 transition-colors duration-150 group-hover:text-text-tertiary"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M8 3v10M3 8h10" />
          </svg>
          Open folder
        </button>
      </div>
    </aside>
  );
}
