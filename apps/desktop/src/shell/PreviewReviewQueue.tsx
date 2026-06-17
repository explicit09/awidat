// Review queue — the elastic block under the program monitor. Absorbs
// whatever vertical space the fitted picture doesn't need:
//
//   • with pending changes: a grid of cut cards (timecode + label,
//     click to jump the playhead) plus Accept all / Reject all;
//   • with none: episode facts + the project-aware starter prompts so
//     the space invites the first agent pass instead of sitting empty.
//
// Scrolls internally when the window is short; the grid reflows via
// auto-fill so wide windows show more cards per row.

import { cn } from "../ui";
import { useStarterPrompts } from "../agent/useStarterPrompts";
import type { PreviewChange } from "./PreviewSurface";

export function PreviewReviewQueue({
  changes,
  activeChangeId,
  durationLabel,
  onSelectChange,
  onAcceptProposal,
  onRejectProposal,
}: {
  changes: PreviewChange[];
  activeChangeId?: string;
  durationLabel: string;
  onSelectChange?: (change: PreviewChange) => void;
  onAcceptProposal?: () => void;
  onRejectProposal?: () => void;
}) {
  if (changes.length === 0) {
    return <QueueEmptyState />;
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex h-6 shrink-0 items-center px-0.5">
        <span className="font-mono text-[10px] font-semibold uppercase tracking-[0.14em] text-[var(--color-text-muted)]">
          Review queue · {changes.length} cut{changes.length === 1 ? "" : "s"}
        </span>
        <span className="ml-auto flex items-center gap-2">
          <span className="font-mono text-[10px] tracking-[0.05em] text-[var(--color-text-muted)]">
            {durationLabel}
          </span>
          <QueueAction label="Accept all" tone="gold" onClick={onAcceptProposal} />
          <QueueAction label="Reject all" onClick={onRejectProposal} />
        </span>
      </div>
      <div className="grid min-h-0 flex-1 auto-rows-min grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-2 overflow-y-auto">
        {changes.map((c) => (
          <button
            key={c.id}
            type="button"
            onClick={() => onSelectChange?.(c)}
            className={cn(
              "flex flex-col items-start gap-1 rounded-[10px] border p-2.5 text-left transition-colors",
              c.id === activeChangeId
                ? "border-[rgba(217,165,75,0.55)] bg-[rgba(217,165,75,0.1)]"
                : "border-[var(--color-border-subtle)] bg-[rgba(255,255,255,0.035)] hover:border-[rgba(217,165,75,0.4)] hover:bg-[rgba(217,165,75,0.06)]",
            )}
          >
            <span className="font-mono text-[10px] font-semibold text-[#e8c040]">
              {String(c.index).padStart(2, "0")} · {formatQueueTime(c.timeS)}
            </span>
            <span className="text-[12px] font-semibold leading-snug text-[var(--color-text-primary)]">
              {c.label ?? queueKindLabel(c.kind)}
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

function QueueAction({
  label,
  tone,
  onClick,
}: {
  label: string;
  tone?: "gold";
  onClick?: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={!onClick}
      className={cn(
        "h-6 rounded-md border px-2.5 text-[11px] font-bold transition-colors disabled:cursor-not-allowed disabled:opacity-40",
        tone === "gold"
          ? "border-[rgba(217,165,75,0.4)] bg-[rgba(217,165,75,0.1)] text-[#e8c040] hover:bg-[rgba(217,165,75,0.18)]"
          : "border-[var(--color-border-subtle)] bg-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]",
      )}
    >
      {label}
    </button>
  );
}

// One slim row anchored directly under the transport — never centered
// in the leftover space (content floating mid-void reads as lost) and
// no container chrome (a boxed zone reads as a hollow rectangle). The
// space below stays plain backdrop.
function QueueEmptyState() {
  const { prompts, running, hasProject, send } = useStarterPrompts();
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className="flex flex-wrap items-center gap-2 px-0.5">
        <span className="text-[11.5px] text-[var(--color-text-muted)]">
          Nothing to review yet —
        </span>
        {prompts.map((p) => (
          <button
            key={p}
            type="button"
            onClick={() => void send(p)}
            disabled={running || !hasProject}
            className="inline-flex h-7 items-center gap-1.5 rounded-full border border-[var(--color-border-subtle)] bg-[rgba(255,255,255,0.04)] px-3 text-[11.5px] font-semibold text-[var(--color-text-secondary)] transition-colors hover:border-[rgba(239,68,68,0.45)] hover:bg-[rgba(239,68,68,0.08)] hover:text-[var(--color-text-primary)] disabled:cursor-not-allowed disabled:opacity-50"
          >
            <span className="text-[var(--color-brand)]">▸</span>
            <span>{p}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

function queueKindLabel(kind: string): string {
  switch (kind) {
    case "cut":
      return "Cut";
    case "tighten":
      return "Tighten";
    default:
      return kind.charAt(0).toUpperCase() + kind.slice(1);
  }
}

function formatQueueTime(totalSeconds: number): string {
  if (!Number.isFinite(totalSeconds) || totalSeconds < 0) return "0:00";
  const h = Math.floor(totalSeconds / 3600);
  const m = Math.floor((totalSeconds % 3600) / 60);
  const s = Math.floor(totalSeconds % 60);
  return h > 0
    ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`
    : `${m}:${String(s).padStart(2, "0")}`;
}
