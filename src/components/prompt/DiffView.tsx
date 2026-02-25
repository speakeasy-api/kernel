import { useState, useEffect, useCallback, useMemo } from "react";
import type { BundledLanguage } from "shiki";
import { cn } from "../../lib/cn";
import { revertFile, type RevertResult } from "../../lib/commands";
import { getHighlighter, langFromPath } from "../../lib/highlighter";
import type { DiffHunk } from "../../hooks/useLlmStream";

interface DiffViewProps {
  path: string;
  status: "created" | "modified";
  hunks: DiffHunk[];
  beforeContent: string | null;
  afterContent: string;
  sessionId: string;
  toolUseId: string;
  isReverted: boolean;
  revertReason?: string;
}

export function DiffView({
  path,
  status,
  hunks,
  beforeContent,
  afterContent,
  sessionId,
  toolUseId,
  isReverted,
  revertReason,
}: DiffViewProps) {
  const [expanded, setExpanded] = useState(true);
  const [reverting, setReverting] = useState(false);
  const [showRevertInput, setShowRevertInput] = useState(false);
  const [revertReasonInput, setRevertReasonInput] = useState("");
  const [revertError, setRevertError] = useState<string | null>(null);

  const stats = useMemo(() => {
    let adds = 0;
    let removes = 0;
    for (const hunk of hunks) {
      for (const line of hunk.lines) {
        if (line.kind === "add") adds++;
        else if (line.kind === "remove") removes++;
      }
    }
    return { adds, removes };
  }, [hunks]);

  const handleRevert = useCallback(async (force = false) => {
    setReverting(true);
    setRevertError(null);
    try {
      const result: RevertResult = await revertFile(
        sessionId,
        toolUseId,
        path,
        beforeContent,
        afterContent,
        revertReasonInput,
        force,
      );
      if (result.status === "conflict") {
        setRevertError("File has been modified since this write. Use force revert to override.");
      } else if (result.status === "not_found") {
        setRevertError("File no longer exists on disk.");
      } else if (result.status === "error") {
        setRevertError(result.message);
      } else {
        setShowRevertInput(false);
      }
    } catch (e) {
      setRevertError(String(e));
    } finally {
      setReverting(false);
    }
  }, [sessionId, toolUseId, path, beforeContent, afterContent, revertReasonInput]);

  return (
    <div
      className={cn(
        "rounded-lg border overflow-hidden transition-opacity duration-200",
        isReverted
          ? "border-border-subtle/50 opacity-60"
          : "border-border-subtle",
      )}
    >
      {/* Header */}
      <button
        type="button"
        onClick={() => setExpanded((e) => !e)}
        className="flex w-full items-center gap-2 bg-surface-1 px-3 py-2 text-left transition-colors hover:bg-surface-2"
      >
        {/* Status dot */}
        <span
          className={cn(
            "h-1.5 w-1.5 shrink-0 rounded-full",
            status === "created" ? "bg-emerald-400" : "bg-amber-400",
          )}
        />

        {/* File path */}
        <span className="truncate font-mono text-[12px] text-text-secondary">
          {path}
        </span>

        {/* Stats */}
        {status === "modified" && (stats.adds > 0 || stats.removes > 0) && (
          <span className="shrink-0 font-mono text-[11px]">
            {stats.adds > 0 && (
              <span className="text-emerald-400/70">+{stats.adds}</span>
            )}
            {stats.adds > 0 && stats.removes > 0 && (
              <span className="text-text-ghost mx-0.5">/</span>
            )}
            {stats.removes > 0 && (
              <span className="text-red-400/70">-{stats.removes}</span>
            )}
          </span>
        )}

        {status === "created" && (
          <span className="shrink-0 font-mono text-[10px] text-emerald-400/60">
            new
          </span>
        )}

        <span className="flex-1" />

        {/* Reverted badge */}
        {isReverted && (
          <span className="shrink-0 rounded-full bg-red-500/10 px-2 py-0.5 text-[10px] text-red-400/80">
            reverted{revertReason ? `: ${revertReason}` : ""}
          </span>
        )}

        {/* Revert button */}
        {!isReverted && (
          <span
            role="button"
            onClick={(e) => {
              e.stopPropagation();
              setShowRevertInput((s) => !s);
            }}
            className="shrink-0 rounded px-1.5 py-0.5 text-[10px] text-text-ghost transition-colors hover:bg-red-500/10 hover:text-red-400"
          >
            revert
          </span>
        )}

        {/* Chevron */}
        <svg
          width="12"
          height="12"
          viewBox="0 0 12 12"
          className={cn(
            "shrink-0 text-text-ghost transition-transform duration-150",
            expanded && "rotate-90",
          )}
        >
          <path
            d="M4.5 2.5L8 6L4.5 9.5"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
            fill="none"
          />
        </svg>
      </button>

      {/* Revert input */}
      {showRevertInput && !isReverted && (
        <div className="flex items-center gap-2 border-t border-border-subtle bg-surface-1/50 px-3 py-2">
          <input
            type="text"
            placeholder="Reason (optional)"
            value={revertReasonInput}
            onChange={(e) => setRevertReasonInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") handleRevert();
              if (e.key === "Escape") setShowRevertInput(false);
            }}
            className="flex-1 rounded bg-surface-0 px-2 py-1 font-mono text-[11px] text-text-secondary outline-none ring-1 ring-border-subtle focus:ring-red-500/30"
            autoFocus
          />
          <button
            type="button"
            onClick={() => handleRevert()}
            disabled={reverting}
            className="shrink-0 rounded bg-red-500/10 px-2.5 py-1 text-[11px] text-red-400 transition-colors hover:bg-red-500/20 disabled:opacity-50"
          >
            {reverting ? "reverting..." : "confirm"}
          </button>
          <button
            type="button"
            onClick={() => setShowRevertInput(false)}
            className="shrink-0 rounded px-2 py-1 text-[11px] text-text-ghost hover:text-text-secondary"
          >
            cancel
          </button>
        </div>
      )}

      {/* Revert error */}
      {revertError && (
        <div className="flex items-center gap-2 border-t border-red-500/10 bg-red-500/5 px-3 py-2">
          <span className="flex-1 text-[11px] text-red-400/80">{revertError}</span>
          {revertError.includes("force") && (
            <button
              type="button"
              onClick={() => handleRevert(true)}
              disabled={reverting}
              className="shrink-0 rounded bg-red-500/10 px-2.5 py-1 text-[10px] text-red-400 transition-colors hover:bg-red-500/20"
            >
              force revert
            </button>
          )}
        </div>
      )}

      {/* Body */}
      <div
        className={cn(
          "grid transition-[grid-template-rows] duration-200",
          expanded ? "grid-rows-[1fr]" : "grid-rows-[0fr]",
        )}
      >
        <div className="overflow-hidden">
          {status === "created" ? (
            <NewFileView path={path} content={afterContent} />
          ) : (
            <HunkList path={path} hunks={hunks} />
          )}
        </div>
      </div>
    </div>
  );
}

// ─── New File View ──────────────────────────────────────────────────────────

function NewFileView({ path, content }: { path: string; content: string }) {
  const [html, setHtml] = useState<string | null>(null);

  useEffect(() => {
    let stale = false;
    getHighlighter().then((hl) => {
      if (stale) return;
      const lang = langFromPath(path) as BundledLanguage;
      try {
        const result = hl.codeToHtml(content, { lang, theme: "kernel-dark" });
        setHtml(result);
      } catch {
        // Language not loaded — show plain
        setHtml(null);
      }
    });
    return () => { stale = true; };
  }, [path, content]);

  const lines = content.split("\n");

  if (html) {
    return (
      <div className="overflow-x-auto">
        <div
          className="font-mono text-[12px] leading-[1.6] [&_pre]:!bg-transparent [&_pre]:p-3 [&_code]:!bg-transparent"
          dangerouslySetInnerHTML={{ __html: html }}
        />
      </div>
    );
  }

  return (
    <div className="overflow-x-auto p-3">
      <table className="w-full border-collapse font-mono text-[12px] leading-[1.6]">
        <tbody>
          {lines.map((line, i) => (
            <tr key={i} className="bg-[var(--color-diff-add-bg)]">
              <td className="w-[1%] select-none whitespace-nowrap pr-3 text-right text-[var(--color-diff-add-num)]">
                {i + 1}
              </td>
              <td className="whitespace-pre text-text-secondary">{line}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

// ─── Hunk List (Unified Diff) ───────────────────────────────────────────────

function HunkList({ path, hunks }: { path: string; hunks: DiffHunk[] }) {
  // Syntax-highlight the "new" side (context + add lines)
  const [tokenMap, setTokenMap] = useState<Map<number, TokenSpan[]> | null>(null);

  useEffect(() => {
    let stale = false;

    // Reconstruct the "new" content from hunks to highlight
    const newLines: string[] = [];
    const lineIndexMap: number[] = []; // maps newLines index → global new-line number
    let newLineNum = 0;

    for (const hunk of hunks) {
      const parsed = parseHunkHeader(hunk.header);
      newLineNum = parsed.newStart;
      for (const line of hunk.lines) {
        if (line.kind === "context" || line.kind === "add") {
          newLines.push(line.content);
          lineIndexMap.push(newLineNum);
          newLineNum++;
        } else {
          // remove lines don't appear in "new" file
        }
      }
    }

    if (newLines.length === 0) return;

    getHighlighter().then((hl) => {
      if (stale) return;
      const lang = langFromPath(path) as BundledLanguage;
      try {
        const result = hl.codeToTokens(newLines.join("\n"), {
          lang,
          theme: "kernel-dark",
        });
        const map = new Map<number, TokenSpan[]>();
        for (let i = 0; i < result.tokens.length; i++) {
          const lineNum = lineIndexMap[i];
          if (lineNum !== undefined) {
            map.set(
              lineNum,
              result.tokens[i].map((t) => ({
                content: t.content,
                color: t.color ?? undefined,
              })),
            );
          }
        }
        setTokenMap(map);
      } catch {
        setTokenMap(null);
      }
    });

    return () => { stale = true; };
  }, [path, hunks]);

  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse font-mono text-[12px] leading-[1.6]">
        <tbody>
          {hunks.map((hunk, hi) => {
            const parsed = parseHunkHeader(hunk.header);
            let oldLine = parsed.oldStart;
            let newLine = parsed.newStart;

            return (
              <HunkRows
                key={hi}
                hunk={hunk}
                header={hunk.header}
                startOldLine={oldLine}
                startNewLine={newLine}
                tokenMap={tokenMap}
                isFirst={hi === 0}
              />
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

interface TokenSpan {
  content: string;
  color?: string;
}

function HunkRows({
  hunk,
  header,
  startOldLine,
  startNewLine,
  tokenMap,
  isFirst,
}: {
  hunk: DiffHunk;
  header: string;
  startOldLine: number;
  startNewLine: number;
  tokenMap: Map<number, TokenSpan[]> | null;
  isFirst: boolean;
}) {
  let oldLine = startOldLine;
  let newLine = startNewLine;

  return (
    <>
      {/* Hunk separator */}
      {!isFirst && (
        <tr>
          <td
            colSpan={3}
            className="bg-surface-1 px-3 py-1 text-[11px] text-text-ghost italic"
          >
            {header}
          </td>
        </tr>
      )}
      {hunk.lines.map((line, li) => {
        const isAdd = line.kind === "add";
        const isRemove = line.kind === "remove";

        const oldNum = isAdd ? null : oldLine;
        const newNum = isRemove ? null : newLine;

        if (!isAdd) oldLine++;
        if (!isRemove) newLine++;

        const highlighted = !isRemove && newNum != null ? tokenMap?.get(newNum) : null;

        return (
          <tr
            key={li}
            className={cn(
              isAdd && "bg-[var(--color-diff-add-bg)]",
              isRemove && "bg-[var(--color-diff-remove-bg)]",
            )}
          >
            {/* Old line number */}
            <td
              className={cn(
                "w-[1%] select-none whitespace-nowrap px-2 text-right text-[11px]",
                isAdd
                  ? "text-transparent"
                  : isRemove
                    ? "text-[var(--color-diff-remove-num)]"
                    : "text-text-ghost",
              )}
            >
              {oldNum ?? ""}
            </td>

            {/* New line number */}
            <td
              className={cn(
                "w-[1%] select-none whitespace-nowrap pr-3 text-right text-[11px]",
                isRemove
                  ? "text-transparent"
                  : isAdd
                    ? "text-[var(--color-diff-add-num)]"
                    : "text-text-ghost",
              )}
            >
              {newNum ?? ""}
            </td>

            {/* Content */}
            <td
              className={cn(
                "whitespace-pre border-l-2",
                isAdd && "border-l-[var(--color-diff-add-border)]",
                isRemove && "border-l-[var(--color-diff-remove-border)]",
                !isAdd && !isRemove && "border-l-transparent",
                "pl-3",
              )}
            >
              {highlighted ? (
                <HighlightedLine tokens={highlighted} />
              ) : (
                <span
                  className={cn(
                    isRemove ? "text-red-400/70" : "text-text-secondary",
                  )}
                >
                  {line.content}
                </span>
              )}
            </td>
          </tr>
        );
      })}
    </>
  );
}

function HighlightedLine({ tokens }: { tokens: TokenSpan[] }) {
  return (
    <>
      {tokens.map((t, i) => (
        <span key={i} style={t.color ? { color: t.color } : undefined}>
          {t.content}
        </span>
      ))}
    </>
  );
}

// ─── Utilities ──────────────────────────────────────────────────────────────

function parseHunkHeader(header: string): {
  oldStart: number;
  oldCount: number;
  newStart: number;
  newCount: number;
} {
  // @@ -oldStart,oldCount +newStart,newCount @@
  const match = header.match(/@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/);
  if (!match) return { oldStart: 1, oldCount: 0, newStart: 1, newCount: 0 };
  return {
    oldStart: parseInt(match[1], 10),
    oldCount: parseInt(match[2] ?? "1", 10),
    newStart: parseInt(match[3], 10),
    newCount: parseInt(match[4] ?? "1", 10),
  };
}
