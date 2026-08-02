// Empty state for the agent rail's conversation pane. Renders a brief
// "Montage" intro card followed by four project-aware starter prompts.
// Mounted by ChatStream when `items.length === 0`; the live command-rail
// composer remains in its parent surface.

import { useStarterPrompts } from "./useStarterPrompts";

export function EmptyConversation() {
  const { prompts, typeKey, running, hasProject, send } = useStarterPrompts();

  // TODO: wire real indexing readiness from App-level state. The
  // IndexReadinessSnapshot currently lives as local state in App.tsx;
  // until it's lifted into a store the agent rail can subscribe to we
  // fall back to a project-agnostic opener.
  const allReady = false;
  const opener = allReady
    ? "I've indexed your clip — speech, scenes, color, audio all ready. Tell me how you want this cut, or pick a starting move below."
    : hasProject
      ? "You can send an edit request now. If more indexing context becomes available, Montage will use it in the next turn."
      : "Open or create a project, then tell me how you want it cut, or pick a starting move below.";

  return (
    <div className="flex flex-col h-full">
      <div
        className="m-3 rounded-lg border border-[var(--color-border-subtle)] p-3"
        style={{
          background:
            "linear-gradient(180deg, rgba(239,68,68,0.05), transparent), var(--color-surface-panel)",
        }}
      >
        <div className="mb-1 text-[12px] font-semibold text-[var(--color-text-primary)]">
          <span className="text-[var(--color-brand)]">◆</span> Montage
          <span className="ml-2 font-mono text-[10px] font-normal text-[var(--color-text-muted)]">
            {typeKey ?? "editor"} mode
          </span>
        </div>
        <p className="m-0 text-[11px] leading-snug text-[var(--color-text-secondary)]">
          {opener}
        </p>
      </div>

      <div className="flex flex-col gap-1.5 px-3">
        <div className="text-[9px] font-bold uppercase tracking-[0.08em] text-[var(--color-text-muted)]">
          Try
        </div>
        {prompts.map((p) => (
          <button
            key={p}
            type="button"
            onClick={() => void send(p)}
            disabled={running || !hasProject}
            className="flex items-center gap-2 rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] px-2.5 py-1.5 text-left text-[11px] text-[var(--color-text-secondary)] hover:border-[var(--color-border)] hover:bg-[var(--color-surface-card-hover)] hover:text-[var(--color-text-primary)] disabled:cursor-not-allowed disabled:opacity-50"
          >
            <span className="text-[var(--color-brand)]">▸</span>
            <span>{p}</span>
          </button>
        ))}
      </div>
    </div>
  );
}
