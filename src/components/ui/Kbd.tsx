import { cn } from "../../lib/cn";

interface KbdProps {
  children: React.ReactNode;
  className?: string;
}

export function Kbd({ children, className }: KbdProps) {
  return (
    <kbd
      className={cn(
        "inline-flex items-center rounded border border-border-default bg-surface-2 px-1 py-0.5 text-[10px] font-mono text-text-ghost",
        className,
      )}
    >
      {children}
    </kbd>
  );
}
