import { useCallback, useEffect, useState } from "react";
import type { Session } from "../lib/types";
import { listSessions, eventsSince } from "../lib/commands";

export interface SessionSummary {
  session: Session;
  /** First user prompt text, or null if none recorded yet. */
  title: string | null;
}

export interface WorkspaceGroup {
  projectPath: string;
  projectName: string;
  sessions: SessionSummary[];
}

function projectName(path: string): string {
  return path.split("/").filter(Boolean).pop() ?? path;
}

async function fetchSummaries(sessions: Session[]): Promise<SessionSummary[]> {
  return Promise.all(
    sessions.map(async (session) => {
      try {
        const events = await eventsSince(session.id, "2000-01-01T00:00:00");
        const promptEvent = events.find((e) => e.kind === "PromptSubmitted");
        let title: string | null = null;
        if (promptEvent) {
          const parsed = JSON.parse(promptEvent.data) as { prompt?: string };
          title = parsed.prompt?.trim() ?? null;
        }
        return { session, title };
      } catch {
        return { session, title: null };
      }
    }),
  );
}

function groupByProject(summaries: SessionSummary[]): WorkspaceGroup[] {
  const map = new Map<string, SessionSummary[]>();
  for (const s of summaries) {
    // Skip empty conversations (no prompt ever submitted)
    if (!s.title) continue;
    const path = s.session.project_path;
    if (!map.has(path)) map.set(path, []);
    map.get(path)!.push(s);
  }

  const groups: WorkspaceGroup[] = [];
  for (const [path, sessions] of map) {
    groups.push({
      projectPath: path,
      projectName: projectName(path),
      // sessions are already DESC from the backend; keep that order
      sessions,
    });
  }

  // Sort groups by most-recent session created_at
  groups.sort((a, b) => {
    const aLatest = a.sessions[0]?.session.created_at ?? "";
    const bLatest = b.sessions[0]?.session.created_at ?? "";
    return bLatest.localeCompare(aLatest);
  });

  return groups;
}

export function useSessionSummaries() {
  const [groups, setGroups] = useState<WorkspaceGroup[]>([]);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const sessions = await listSessions();
      const summaries = await fetchSummaries(sessions);
      setGroups(groupByProject(summaries));
    } catch {
      // leave previous state intact on error
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  return { groups, loading, refresh };
}
