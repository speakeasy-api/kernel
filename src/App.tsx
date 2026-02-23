import { useState, useCallback, useRef } from "react";
import type { Session } from "./lib/types";
import { deleteSession, eventsSince } from "./lib/commands";
import { PromptWindow } from "./components/prompt/PromptWindow";
import { WorkspaceSelector } from "./components/workspace/WorkspaceSelector";
import { Sidebar } from "./components/sidebar/Sidebar";
import { useModes } from "./hooks/useModes";
import { useConfig } from "./hooks/useConfig";

type View =
  | { kind: "workspace-selector" }
  | { kind: "prompt-window"; session: Session };

function App() {
  const [view, setView] = useState<View>({ kind: "workspace-selector" });
  const [sidebarRefreshKey, setSidebarRefreshKey] = useState(0);
  const { modes } = useModes();
  const config = useConfig(
    view.kind === "prompt-window" ? view.session.project_path : null,
  );

  // Track which session we might need to clean up
  const currentSessionRef = useRef<Session | null>(null);
  if (view.kind === "prompt-window") {
    currentSessionRef.current = view.session;
  }

  const activeSessionId =
    view.kind === "prompt-window" ? view.session.id : null;

  const handleSelectSession = useCallback(async (session: Session) => {
    const prev = currentSessionRef.current;
    setView({ kind: "prompt-window", session });

    // Clean up the previous session if it was empty
    if (prev && prev.id !== session.id) {
      try {
        const events = await eventsSince(prev.id, "2000-01-01T00:00:00");
        const hasPrompt = events.some((e) => e.kind === "PromptSubmitted");
        if (!hasPrompt) {
          await deleteSession(prev.id);
          setSidebarRefreshKey((k) => k + 1);
        }
      } catch {
        // ignore
      }
    }
  }, []);

  const handleNewSession = useCallback(async (session: Session) => {
    const prev = currentSessionRef.current;
    setView({ kind: "prompt-window", session });

    // Clean up the previous session if it was empty
    if (prev && prev.id !== session.id) {
      try {
        const events = await eventsSince(prev.id, "2000-01-01T00:00:00");
        const hasPrompt = events.some((e) => e.kind === "PromptSubmitted");
        if (!hasPrompt) {
          await deleteSession(prev.id);
        }
      } catch {
        // ignore
      }
    }
    setSidebarRefreshKey((k) => k + 1);
  }, []);

  const handleSessionCreated = useCallback((session: Session) => {
    setSidebarRefreshKey((k) => k + 1);
    setView({ kind: "prompt-window", session });
  }, []);

  // When closing a session, check if it's empty and clean up
  const handleClose = useCallback(async () => {
    const session = currentSessionRef.current;
    setView({ kind: "workspace-selector" });
    currentSessionRef.current = null;

    if (session) {
      try {
        const events = await eventsSince(session.id, "2000-01-01T00:00:00");
        const hasPrompt = events.some((e) => e.kind === "PromptSubmitted");
        if (!hasPrompt) {
          await deleteSession(session.id);
        }
      } catch {
        // ignore cleanup errors
      }
      setSidebarRefreshKey((k) => k + 1);
    }
  }, []);

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-surface-0">
      <Sidebar
        activeSessionId={activeSessionId}
        onSelectSession={handleSelectSession}
        onNewSession={handleNewSession}
        refreshKey={sidebarRefreshKey}
      />

      <main className="relative flex flex-1 flex-col overflow-hidden">
        {view.kind === "prompt-window" ? (
          <PromptWindow
            key={view.session.id}
            session={view.session}
            modes={modes}
            config={config}
            onClose={handleClose}
          />
        ) : (
          <WorkspaceSelector onSessionCreated={handleSessionCreated} />
        )}
      </main>
    </div>
  );
}

export default App;
