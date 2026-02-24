import { useState } from "react";

interface ModelBadgeProps {
  model: string;
}

export function ModelBadge({ model }: ModelBadgeProps) {
  const [copied, setCopied] = useState(false);

  const handleCopy = () => {
    navigator.clipboard.writeText(model).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };

  return (
    <span className="inline-flex items-center gap-1.5 text-[11px] font-mono text-text-ghost tracking-tight">
      <span
        className="h-1.5 w-1.5 rounded-full animate-pulse-subtle"
        style={{ backgroundColor: `var(--mode-tint)` }}
      />
      {model}
      <button
        onClick={handleCopy}
        className="ml-0.5 text-text-ghost hover:text-text-secondary transition-colors cursor-pointer"
        title="Copy model name"
      >
        {copied ? (
          <svg className="h-3 w-3" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <path d="M2 6l3 3 5-5" />
          </svg>
        ) : (
          <svg className="h-3 w-3" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <rect x="4" y="4" width="6.5" height="6.5" rx="1" />
            <path d="M8 4V2.5A1 1 0 007 1.5H2.5A1 1 0 001.5 2.5V7A1 1 0 002.5 8H4" />
          </svg>
        )}
      </button>
    </span>
  );
}
