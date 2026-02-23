import type { Session } from "../../lib/types";

interface WorkspaceSelectorProps {
  onSessionCreated: (session: Session) => void;
}

export function WorkspaceSelector(_props: WorkspaceSelectorProps) {
  return (
    <div className="flex h-full flex-col bg-surface-0">
      {/* Drag region — sits to the right of the sidebar */}
      <div
        className="h-10 shrink-0"
        style={{ WebkitAppRegion: "drag" } as React.CSSProperties}
      />

      <div className="flex flex-1 flex-col items-center justify-center gap-4 px-10 pb-12">
        {/* Logo */}
        <div className="mb-2">
          <h1 className="text-[22px] font-semibold tracking-[-0.02em] text-text-primary">
            kernel
          </h1>
        </div>

        <p className="text-center text-[13px] leading-relaxed text-text-ghost max-w-[220px]">
          Select a conversation from the sidebar, or open a folder to start a
          new one.
        </p>

        {/* Keyboard shortcut hint */}
        <div className="mt-2 flex items-center gap-1.5 text-[11px] text-text-ghost">
          <span>Use the sidebar to browse workspaces</span>
        </div>
      </div>
    </div>
  );
}
