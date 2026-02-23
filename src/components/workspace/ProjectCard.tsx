import { cn } from "../../lib/cn";

interface ProjectCardProps {
  name: string;
  path: string;
  timeAgo: string;
  onClick: () => void;
  index: number;
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

export function ProjectCard({
  name,
  path,
  timeAgo,
  onClick,
  index,
}: ProjectCardProps) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "group flex w-full items-center gap-3 rounded-lg px-2.5 py-2",
        "text-left transition-all duration-100 cursor-pointer",
        "hover:bg-surface-2/80 active:bg-surface-3/50",
      )}
      style={{ animationDelay: `${index * 30}ms` }}
    >
      <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-surface-3/60 text-[12px] font-semibold text-text-tertiary transition-colors duration-100 group-hover:bg-surface-4 group-hover:text-text-secondary">
        {name.charAt(0).toUpperCase()}
      </div>
      <div className="min-w-0 flex-1">
        <p className="truncate text-[13px] font-medium leading-tight text-text-primary">
          {name}
        </p>
        <p className="mt-0.5 truncate text-[11px] leading-tight text-text-ghost font-mono">
          {shortenPath(path)}
        </p>
      </div>
      <span className="shrink-0 text-[10px] font-mono text-text-ghost tabular-nums">
        {timeAgo}
      </span>
    </button>
  );
}
