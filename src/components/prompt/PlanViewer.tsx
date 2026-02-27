import { useEffect, useState, useCallback } from "react";
import { readPlanContent } from "../../lib/commands";
import { MarkdownMessage } from "./MarkdownMessage";

interface PlanViewerProps {
  sessionId: string;
  filename: string;
  onClose: () => void;
}

export function PlanViewer({ sessionId, filename, onClose }: PlanViewerProps) {
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    setLoading(true);
    readPlanContent(sessionId)
      .then((c) => setContent(c))
      .catch(() => setContent(null))
      .finally(() => setLoading(false));
  }, [sessionId]);

  const handleBackdropClick = useCallback(
    (e: React.MouseEvent) => {
      if (e.target === e.currentTarget) onClose();
    },
    [onClose],
  );

  useEffect(() => {
    function handleKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [onClose]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/60 backdrop-blur-sm pt-[10vh]"
      onClick={handleBackdropClick}
    >
      <div className="relative w-full max-w-[600px] max-h-[75vh] rounded-xl border border-border-subtle bg-surface-0 shadow-2xl flex flex-col overflow-hidden mx-4">
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-border-subtle shrink-0">
          <div className="flex items-center gap-2 min-w-0">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" className="shrink-0 text-accent">
              <path d="M4 1h5.586a1 1 0 0 1 .707.293l3.414 3.414a1 1 0 0 1 .293.707V14a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V2a1 1 0 0 1 1-1Z" stroke="currentColor" strokeWidth="1.2" />
              <path d="M5.5 8h5M5.5 10.5h5M5.5 5.5h2" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
            </svg>
            <span className="text-[12px] font-mono text-text-secondary truncate">
              {filename}
            </span>
          </div>
          <button
            onClick={onClose}
            className="text-text-ghost hover:text-text-secondary transition-colors p-1 -mr-1"
          >
            <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
              <path d="M3 3l8 8M11 3l-8 8" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            </svg>
          </button>
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto px-5 py-4">
          {loading ? (
            <div className="flex items-center justify-center py-8">
              <span className="text-[11px] text-text-ghost animate-pulse">Loading plan...</span>
            </div>
          ) : content ? (
            <div className="text-[14px] leading-relaxed text-text-secondary">
              <MarkdownMessage content={content} role="assistant" />
            </div>
          ) : (
            <div className="flex items-center justify-center py-8">
              <span className="text-[11px] text-text-ghost">Plan not found</span>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
