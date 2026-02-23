import { useEffect, useRef, useState } from "react";
import { cn } from "../../lib/cn";
import { getModeTint } from "../../lib/modeTint";
import type { Mode } from "../../lib/types";

interface ModeSelectorProps {
  modes: Mode[];
  selected: Mode;
  onSelect: (mode: Mode) => void;
}

const AUTO_MODE: Mode = {
  name: "auto",
  description: "Let Kernel choose",
  system_prompt: "",
  default_model: null,
  allowed_tools: [],
  created_by: "builtin",
  version: 1,
};

export function ModeSelector({ modes, selected, onSelect }: ModeSelectorProps) {
  const [isOpen, setIsOpen] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);
  const allModes = [AUTO_MODE, ...modes];

  // Track mode switches for animation
  const [animKey, setAnimKey] = useState(0);
  const prevModeRef = useRef(selected.name);
  useEffect(() => {
    if (selected.name !== prevModeRef.current) {
      prevModeRef.current = selected.name;
      setAnimKey((k) => k + 1);
    }
  }, [selected.name]);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape" && isOpen) setIsOpen(false);
      if ((e.metaKey || e.ctrlKey) && e.key === "k") {
        e.preventDefault();
        setIsOpen((prev) => !prev);
      }
    }
    function handleClickOutside(e: MouseEvent) {
      if (
        containerRef.current &&
        !containerRef.current.contains(e.target as Node)
      )
        setIsOpen(false);
    }
    document.addEventListener("keydown", handleKeyDown);
    document.addEventListener("mousedown", handleClickOutside);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      document.removeEventListener("mousedown", handleClickOutside);
    };
  }, [isOpen]);

  const selectedTint = getModeTint(selected.name);

  return (
    <div ref={containerRef} className="relative">
      <button
        key={animKey}
        onClick={() => setIsOpen((prev) => !prev)}
        className={cn(
          "inline-flex items-center gap-1.5 rounded-lg px-2 py-1 text-[12px] font-medium tracking-tight cursor-pointer",
          "transition-all duration-100",
          isOpen
            ? "bg-surface-3 text-text-primary"
            : "text-text-tertiary hover:text-text-secondary hover:bg-surface-2",
        )}
        style={{
          animation: animKey > 0
            ? "mode-switch 500ms cubic-bezier(0.16, 1, 0.3, 1) both, mode-glow 800ms ease-out both"
            : undefined,
        }}
      >
        <span
          className="flex h-4 w-4 items-center justify-center rounded text-[9px] font-bold transition-colors duration-300"
          style={{
            backgroundColor: `hsla(${selectedTint.hue}, 40%, 50%, ${animKey > 0 ? 0.3 : 0.15})`,
            color: `hsl(${selectedTint.hue} 50% 65%)`,
          }}
        >
          {selected.name.charAt(0).toUpperCase()}
        </span>
        <span className="transition-colors duration-300">
          {selected.name}
        </span>
        <svg
          className={cn(
            "h-2.5 w-2.5 text-text-ghost transition-transform duration-150",
            isOpen && "rotate-180",
          )}
          viewBox="0 0 10 10"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M2.5 4L5 6.5 7.5 4" />
        </svg>
      </button>

      {isOpen && (
        <div className="absolute bottom-full left-0 mb-2 w-56 rounded-xl border border-border-default bg-surface-1 p-1 shadow-[0_16px_48px_-12px_rgba(0,0,0,0.6)] z-50 animate-popover">
          {allModes.map((mode) => {
            const t = getModeTint(mode.name);
            const isSelected = mode.name === selected.name;
            return (
              <button
                key={mode.name}
                onClick={() => {
                  onSelect(mode);
                  setIsOpen(false);
                }}
                className={cn(
                  "flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left cursor-pointer",
                  "transition-colors duration-75",
                  isSelected
                    ? "text-text-primary"
                    : "text-text-secondary hover:bg-surface-2 hover:text-text-primary",
                )}
                style={isSelected ? {
                  backgroundColor: `hsla(${t.hue}, 40%, 50%, 0.08)`,
                } : undefined}
              >
                <span
                  className="flex h-5 w-5 shrink-0 items-center justify-center rounded-md text-[10px] font-bold"
                  style={{
                    backgroundColor: `hsla(${t.hue}, 40%, 50%, ${isSelected ? 0.2 : 0.1})`,
                    color: `hsl(${t.hue} ${isSelected ? 55 : 35}% ${isSelected ? 65 : 55}%)`,
                  }}
                >
                  {mode.name.charAt(0).toUpperCase()}
                </span>
                <div className="flex-1 min-w-0">
                  <p className="text-[12px] font-medium leading-none">
                    {mode.name}
                  </p>
                  {mode.description && (
                    <p className="mt-0.5 text-[10px] text-text-ghost truncate leading-tight">
                      {mode.description}
                    </p>
                  )}
                </div>
                {isSelected && (
                  <svg
                    className="h-3 w-3 shrink-0"
                    style={{ color: `hsl(${t.hue} 50% 65%)` }}
                    viewBox="0 0 12 12"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                  >
                    <path d="M2 6.5l3 3 5-6.5" />
                  </svg>
                )}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
