import { useCallback, useEffect, useRef, useState } from "react";
import { cn } from "../../lib/cn";

interface PromptInputProps {
  value: string;
  onChange: (value: string) => void;
  onSubmit: () => void;
  busy?: boolean;
  onCancel?: () => void;
  pinned: boolean;
  onPinToggle: () => void;
}

export function PromptInput({ value, onChange, onSubmit, busy, onCancel, pinned, onPinToggle }: PromptInputProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const [focused, setFocused] = useState(false);

  const resize = useCallback(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    const maxH = 20 * 10;
    el.style.height = `${Math.min(el.scrollHeight, maxH)}px`;
  }, []);

  useEffect(() => {
    resize();
  }, [value, resize]);

  useEffect(() => {
    const t = setTimeout(() => textareaRef.current?.focus(), 200);
    return () => clearTimeout(t);
  }, []);

  function handleKeyDown(e: React.KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      onSubmit();
    }
    if (e.key === "Escape" && busy && onCancel) {
      e.preventDefault();
      onCancel();
    }
  }

  const hasValue = value.trim().length > 0;

  return (
    <div
      className={cn(
        "group relative rounded-2xl border bg-surface-1 transition-all duration-200",
        focused
          ? "shadow-[0_0_0_1px_var(--color-border-default),0_8px_40px_-12px_rgba(0,0,0,0.5)]"
          : "shadow-[0_2px_20px_-4px_rgba(0,0,0,0.3)]",
      )}
      style={{
        borderColor: `var(--mode-tint-muted)`,
        ...(focused ? {
          boxShadow: `0 0 0 1px var(--mode-tint-subtle), 0 8px 40px -12px rgba(0,0,0,0.5), 0 0 20px -4px var(--mode-tint-glow)`,
        } : {}),
      }}
    >
      {/* Pin toggle */}
      <div className="absolute left-3 bottom-3">
        <button
          type="button"
          onClick={onPinToggle}
          disabled={busy}
          className={cn(
            "flex h-7 w-7 items-center justify-center rounded-lg transition-all duration-150 cursor-pointer",
            pinned
              ? "text-amber-400 bg-amber-400/10"
              : "text-text-ghost hover:text-text-secondary",
            busy && "opacity-40 cursor-not-allowed",
          )}
          title={pinned ? "Unpin message" : "Pin message (survives compaction)"}
        >
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
            <path
              d="M9.828 1.172a1 1 0 0 1 1.414 0l3.586 3.586a1 1 0 0 1 0 1.414l-2.293 2.293-.707.707-1.414-1.414-2.828 2.828L7 14l-1-1-3.586-3.586L1.5 8.5l3.414.414 2.828-2.828-1.414-1.414.707-.707z"
              stroke="currentColor"
              strokeWidth="1.2"
              strokeLinecap="round"
              strokeLinejoin="round"
              fill={pinned ? "currentColor" : "none"}
            />
          </svg>
        </button>
      </div>

      <textarea
        ref={textareaRef}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={handleKeyDown}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        placeholder="What do you want to build?"
        rows={1}
        autoCorrect="off"
        autoCapitalize="off"
        autoComplete="off"
        spellCheck={false}
        className={cn(
          "w-full resize-none bg-transparent pl-11 pr-14 pt-4 pb-3",
          "text-[15px] leading-6 text-text-primary",
          "placeholder:text-text-ghost placeholder:font-light",
          "focus:outline-none",
          "[&::-webkit-scrollbar]:hidden [-ms-overflow-style:none] [scrollbar-width:none]",
        )}
      />

      {/* Submit / Cancel */}
      <div className="absolute right-3 bottom-3">
        {busy ? (
          <button
            onClick={onCancel}
            className="flex h-7 items-center justify-center gap-1 rounded-lg px-2 transition-all duration-150 hover:brightness-110 active:scale-95 cursor-pointer"
            style={{ backgroundColor: `var(--mode-tint)`, color: "white" }}
          >
            <span className="text-[12px] font-mono leading-none">esc</span>
          </button>
        ) : (
          <div
            className={cn(
              "transition-all duration-200",
              hasValue
                ? "opacity-100 translate-y-0"
                : "opacity-0 translate-y-1 pointer-events-none",
            )}
          >
            <button
              onClick={onSubmit}
              disabled={!hasValue}
              className="flex h-7 items-center justify-center gap-0.5 rounded-lg px-1.5 transition-all duration-150 hover:brightness-110 active:scale-95 cursor-pointer"
              style={{ backgroundColor: `var(--mode-tint)`, color: "white" }}
            >
              <span className="text-[13px] font-mono leading-none">&#x2318;</span>
              <span className="text-[13px] font-mono leading-none relative -top-px">&#x21B5;</span>
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
