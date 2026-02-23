import { cn } from "../../lib/cn";

type Variant = "primary" | "secondary" | "ghost";

interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
}

const variants: Record<Variant, string> = {
  primary:
    "bg-accent text-white hover:bg-accent-dim active:scale-[0.98]",
  secondary:
    "border border-border-default text-text-secondary hover:bg-surface-2 hover:border-border-strong hover:text-text-primary active:bg-surface-3",
  ghost:
    "text-text-tertiary hover:bg-surface-2 hover:text-text-secondary active:bg-surface-3",
};

export function Button({
  variant = "primary",
  className,
  ...props
}: ButtonProps) {
  return (
    <button
      className={cn(
        "inline-flex items-center justify-center gap-2 rounded-lg px-3 py-1.5 text-[13px] font-medium transition-all duration-100 cursor-pointer disabled:opacity-30 disabled:pointer-events-none",
        variants[variant],
        className,
      )}
      {...props}
    />
  );
}
