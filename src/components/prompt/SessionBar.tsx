import type { Session } from "../../lib/types";

interface SessionBarProps {
  session: Session;
  onClose: () => void;
}

function folderName(path: string): string {
  return path.split("/").pop() || path;
}

function shortenPath(path: string): string {
  const home = "/Users/";
  if (path.startsWith(home)) {
    const rest = path.slice(home.length);
    const slash = rest.indexOf("/");
    return "~" + (slash >= 0 ? rest.slice(slash) : "/" + rest);
  }
  return path;
}

export function SessionBar({ session, onClose }: SessionBarProps) {
  return (
    <div
      className="flex h-10 shrink-0 items-center justify-between px-3"
      style={{ WebkitAppRegion: "drag" } as React.CSSProperties}
    >
      <div
        className="flex items-center gap-2 overflow-hidden"
        style={{ WebkitAppRegion: "no-drag" } as React.CSSProperties}
      >
        <span className="truncate text-[12px] font-medium text-text-secondary tracking-tight">
          {folderName(session.project_path)}
        </span>
        <span className="shrink-0 text-[11px] text-text-ghost font-mono">
          {shortenPath(session.project_path)}
        </span>
      </div>

      <button
        onClick={onClose}
        className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-text-ghost transition-colors duration-100 hover:bg-surface-3 hover:text-text-secondary cursor-pointer"
        style={{ WebkitAppRegion: "no-drag" } as React.CSSProperties}
        aria-label="Close session"
      >
        <svg
          className="h-3 w-3"
          viewBox="0 0 12 12"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
        >
          <path d="M3 3l6 6M9 3l-6 6" />
        </svg>
      </button>
    </div>
  );
}
