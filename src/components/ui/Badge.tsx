import { cn } from "../../lib/cn";

interface BadgeProps {
  children: React.ReactNode;
  className?: string;
}

export function Badge({ children, className }: BadgeProps) {
  return (
    <span
      className={cn(
        "inline-flex items-center rounded-md bg-surface-3 px-1.5 py-0.5 text-[10px] font-medium text-text-tertiary font-mono",
        className,
      )}
    >
      {children}
    </span>
  );
}
