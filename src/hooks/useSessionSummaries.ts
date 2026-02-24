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
  // Collect all unique project paths (so workspaces always appear)
  const pathSet = new Set<string>();
  const nonEmpty = new Map<string, SessionSummary[]>();

  for (const s of summaries) {
    const path = s.session.project_path;
    pathSet.add(path);
    // Only include conversations that have at least one prompt
    if (s.title) {
      if (!nonEmpty.has(path)) nonEmpty.set(path, []);
      nonEmpty.get(path)!.push(s);
    }
  }

  const groups: WorkspaceGroup[] = [];
  for (const path of pathSet) {
    groups.push({
      projectPath: path,
      projectName: projectName(path),
      sessions: nonEmpty.get(path) ?? [],
    });
  }

  // Sort groups by most-recent session created_at (from any session, including empty)
  const latestByPath = new Map<string, string>();
  for (const s of summaries) {
    const path = s.session.project_path;
    const current = latestByPath.get(path) ?? "";
    if (s.session.created_at > current) {
      latestByPath.set(path, s.session.created_at);
    }
  }

  groups.sort((a, b) => {
    const aLatest = latestByPath.get(a.projectPath) ?? "";
    const bLatest = latestByPath.get(b.projectPath) ?? "";
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
