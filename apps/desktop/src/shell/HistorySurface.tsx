/**
 * HistorySurface — the workspace surface for the History tab (Wave 3 T4,
 * Wave 4 W4.5 wired Restore for real).
 *
 * Renders the per-project log of proposal decisions from
 * `useProposalHistoryStore`: every accept, reject, revise, and restore
 * event is surfaced as a chronological row. The data IS the project's
 * history — the propose → ghost → decide loop is Montage's editorial
 * primitive, so the audit trail is the "git log" for a video edit.
 *
 * Layout:
 *   [ filter chips ][ search ]
 *   ──────────────────────────────
 *   | row 1 — newest decision    |
 *   | row 2                      |
 *   | ...                        |
 *
 * Each row carries: relative timestamp, medium chip, source pill,
 * title, rationale (muted), decision badge, and a "↺ Restore" button
 * on rows that carry an inline OTIO snapshot (accepted + restored).
 * Restore confirms via a token-styled modal, dispatches
 * `restore_timeline_otio` to write the snapshot back, then records a
 * "restored" entry so the action itself is reversible.
 */

import { useEffect, useMemo, useState } from "react";
import { History as HistoryIcon, RotateCcw, Search, Sparkles, User } from "lucide-react";

import { cn } from "../ui";
import { useProjectStore } from "../app/state";
import { MEDIUM_STYLE } from "./brief/ProposalCard";
import { PatternsPanel } from "./PatternsPanel";
import { synthesizeLearnedPatterns } from "../state/learnedPatterns";
import {
  buildRestoreEntry,
  useProposalHistoryStore,
  type HistoryDecision,
  type HistoryEntry,
} from "../state/proposalHistory";
import { useBriefProposalsStore, type BriefMedium, type BriefProposalSource } from "../state/briefProposals";
import { useTimelineStore } from "../timeline/store";

type DecisionFilter = "all" | HistoryDecision;
type MediumFilter = "all" | BriefMedium;

// Medium chips the user can toggle. Order matches the Brief stack
// rollups so muscle memory carries over. `mixed` and `other` are
// excluded from the chip row — they're rare and clutter the bar.
const MEDIUM_FILTERS: { id: MediumFilter; label: string }[] = [
  { id: "all", label: "All media" },
  { id: "cut", label: "Cut" },
  { id: "color", label: "Color" },
  { id: "broll", label: "B-roll" },
  { id: "audio", label: "Audio" },
  { id: "title", label: "Caption" },
  { id: "transition", label: "Transition" },
];

const DECISION_FILTERS: { id: DecisionFilter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "accepted", label: "Accepted" },
  { id: "rejected", label: "Rejected" },
  { id: "revised", label: "Revised" },
  { id: "restored", label: "Restored" },
];

/** Decision → token-styled badge. Green/red/amber/blue per spec. */
const DECISION_BADGE: Record<HistoryDecision, { label: string; className: string }> = {
  accepted: {
    label: "Accepted",
    className:
      "bg-[rgba(34,197,94,0.12)] text-[#86EFAC] border-[rgba(34,197,94,0.32)]",
  },
  rejected: {
    label: "Rejected",
    className:
      "bg-[rgba(239,68,68,0.12)] text-[#FCA5A5] border-[rgba(239,68,68,0.32)]",
  },
  revised: {
    label: "Revised",
    className:
      "bg-[rgba(245,158,11,0.12)] text-[#FCD34D] border-[rgba(245,158,11,0.32)]",
  },
  restored: {
    label: "Restored",
    className:
      "bg-[rgba(59,130,246,0.12)] text-[#93C5FD] border-[rgba(59,130,246,0.32)]",
  },
};

/**
 * `cockpit` (default) is the dense workspace layout. `sheet` is the calm,
 * glass-sheet-native decision-stream timeline used by the 2026 Stage shell.
 * Both variants share the SAME data source (`useProposalHistoryStore`) and
 * filter logic — only the chrome differs.
 */
export type HistorySurfaceVariant = "sheet" | "cockpit";

export function HistorySurface({
  variant = "cockpit",
}: {
  variant?: HistorySurfaceVariant;
} = {}) {
  const projectPath = useProjectStore((s) => s.current);
  // Subscribe to entries so re-renders fire on every decision. We
  // re-derive the per-project slice in a memo below.
  const allEntries = useProposalHistoryStore((s) => s.entries);
  const entries = useMemo(
    () => (projectPath ? filterByProject(allEntries, projectPath) : []),
    [allEntries, projectPath],
  );

  const [decisionFilter, setDecisionFilter] = useState<DecisionFilter>("all");
  const [mediumFilter, setMediumFilter] = useState<MediumFilter>("all");
  const [query, setQuery] = useState("");
  const [restoreEntry, setRestoreEntry] = useState<HistoryEntry | null>(null);
  const [restoreToast, setRestoreToast] = useState<string | null>(null);

  // Toast auto-dismiss. 3.5s is the usual sweet spot — long enough to
  // read, short enough that it doesn't linger as visual noise.
  useEffect(() => {
    if (!restoreToast) return;
    const handle = window.setTimeout(() => setRestoreToast(null), 3500);
    return () => window.clearTimeout(handle);
  }, [restoreToast]);

  const filtered = useMemo(
    () => applyFilters(entries, { decisionFilter, mediumFilter, query }),
    [entries, decisionFilter, mediumFilter, query],
  );

  if (variant === "sheet") {
    return (
      <HistorySheet
        entries={entries}
        filtered={filtered}
        decisionFilter={decisionFilter}
        mediumFilter={mediumFilter}
        onDecisionFilter={setDecisionFilter}
        onMediumFilter={setMediumFilter}
        restoreEntry={restoreEntry}
        onOpenRestore={setRestoreEntry}
        onCloseRestore={() => setRestoreEntry(null)}
        restoreToast={restoreToast}
        onRestored={setRestoreToast}
      />
    );
  }

  return (
    <section
      className="panel flex h-full min-h-0 flex-col overflow-hidden"
      aria-label="Decision history"
    >
      <header className="flex shrink-0 flex-col gap-2 border-b border-[var(--color-border-subtle)] px-3 py-2">
        <div className="flex items-center justify-between gap-3">
          <div className="flex items-center gap-2 text-[var(--color-text-primary)]">
            <HistoryIcon className="h-3.5 w-3.5" strokeWidth={1.75} />
            <span className="text-[12px] font-semibold">Decision History</span>
            <span className="font-mono text-[11px] text-[var(--color-text-muted)]">
              {entries.length} total
            </span>
          </div>
          <div className="relative">
            <Search
              className="pointer-events-none absolute left-2 top-1/2 h-3 w-3 -translate-y-1/2 text-[var(--color-text-muted)]"
              strokeWidth={1.75}
            />
            <input
              type="text"
              placeholder="Filter by title or rationale"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              className={cn(
                "h-7 w-64 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)]",
                "bg-[var(--color-surface-input)] pl-7 pr-2 text-[11px]",
                "text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)]",
                "focus:border-[var(--color-brand)] focus:outline-none",
              )}
              aria-label="Filter history"
            />
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          {DECISION_FILTERS.map((f) => (
            <FilterChip
              key={f.id}
              label={f.label}
              active={decisionFilter === f.id}
              onClick={() => setDecisionFilter(f.id)}
            />
          ))}
          <span className="mx-1 h-3 w-px bg-[var(--color-border-subtle)]" aria-hidden />
          {MEDIUM_FILTERS.map((f) => (
            <FilterChip
              key={f.id}
              label={f.label}
              active={mediumFilter === f.id}
              onClick={() => setMediumFilter(f.id)}
            />
          ))}
        </div>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <PatternsPanel entries={entries} />
        {entries.length === 0 ? (
          <EmptyState>
            No decisions yet. Accept or reject a proposal from the Brief
            to start building history.
          </EmptyState>
        ) : filtered.length === 0 ? (
          <EmptyState>No entries match these filters.</EmptyState>
        ) : (
          <ul className="flex flex-col">
            {filtered.map((entry, idx) => (
              <li key={`${entry.id}-${entry.decidedAt}-${idx}`}>
                <HistoryRow
                  entry={entry}
                  onRestore={() => setRestoreEntry(entry)}
                />
              </li>
            ))}
          </ul>
        )}
      </div>

      {restoreEntry && (
        <RestoreDialog
          entry={restoreEntry}
          onClose={() => setRestoreEntry(null)}
          onRestored={(message) => setRestoreToast(message)}
        />
      )}

      {restoreToast && (
        <div
          role="status"
          aria-live="polite"
          className={cn(
            "pointer-events-none absolute bottom-3 left-1/2 z-10",
            "-translate-x-1/2 rounded-[var(--radius-sm)]",
            "border border-[var(--color-border-subtle)]",
            "bg-[var(--color-surface-app)] px-3 py-1.5",
            "text-[11px] text-[var(--color-text-primary)] shadow-lg",
          )}
        >
          {restoreToast}
        </div>
      )}
    </section>
  );
}

/* =====================================================================
   SHEET VARIANT — glass-sheet-native decision-stream timeline
   Calm vertical list of glass rows with a left timeline hairline. Reuses
   the same entries, filters, badge colors, MEDIUM_STYLE, restore flow,
   and relative-time helper as the cockpit; only the chrome differs.
   ===================================================================== */

/** Decision → sheet accent color (mint / red / amber / blue per spec). */
const SHEET_DECISION_COLOR: Record<HistoryDecision, string> = {
  accepted: "#5EEAD4",
  rejected: "#FCA5A5",
  revised: "#FCD34D",
  restored: "#93C5FD",
};

type HistorySheetProps = {
  entries: HistoryEntry[];
  filtered: HistoryEntry[];
  decisionFilter: DecisionFilter;
  mediumFilter: MediumFilter;
  onDecisionFilter: (f: DecisionFilter) => void;
  onMediumFilter: (f: MediumFilter) => void;
  restoreEntry: HistoryEntry | null;
  onOpenRestore: (entry: HistoryEntry) => void;
  onCloseRestore: () => void;
  restoreToast: string | null;
  onRestored: (message: string) => void;
};

function HistorySheet({
  entries,
  filtered,
  decisionFilter,
  mediumFilter,
  onDecisionFilter,
  onMediumFilter,
  restoreEntry,
  onOpenRestore,
  onCloseRestore,
  restoreToast,
  onRestored,
}: HistorySheetProps) {
  // Reuse the existing learned-patterns synthesis for a calm one-line
  // summary — no new computation, no LLM, just the reject log.
  const patterns = useMemo(
    () => synthesizeLearnedPatterns(entries),
    [entries],
  );

  return (
    <section
      className="relative flex h-full min-h-0 flex-col overflow-hidden"
      aria-label="Decision history"
    >
      <header className="shrink-0 px-1 pb-4">
        <div className="flex items-baseline gap-2.5">
          <h2 className="text-[16px] font-semibold tracking-tight text-[var(--color-text-primary)]">
            History
            <span className="text-[var(--color-text-muted)]"> · decisions</span>
          </h2>
          <span className="font-mono text-[12px] text-[var(--color-text-muted)] tabular-nums">
            {entries.length} total
          </span>
        </div>

        <div className="mt-3 flex flex-wrap items-center gap-1.5">
          {DECISION_FILTERS.map((f) => (
            <SheetChip
              key={f.id}
              label={f.label}
              active={decisionFilter === f.id}
              onClick={() => onDecisionFilter(f.id)}
            />
          ))}
          <span
            className="mx-1 h-3.5 w-px bg-[var(--glass-border)]"
            aria-hidden
          />
          {MEDIUM_FILTERS.map((f) => (
            <SheetChip
              key={f.id}
              label={f.label}
              active={mediumFilter === f.id}
              onClick={() => onMediumFilter(f.id)}
            />
          ))}
        </div>

        {patterns.totalRejects > 0 && patterns.mostRejectedMedium && (
          <p className="mt-3 text-[11px] text-[var(--color-text-muted)]">
            <span className="text-[var(--color-text-secondary)]">
              Learned pattern ·
            </span>{" "}
            {patterns.mostRejectedMedium.pct}% of rejections are{" "}
            {MEDIUM_STYLE[patterns.mostRejectedMedium.medium].label} edits
            {patterns.silentRejectPct > 0 &&
              ` · ${patterns.silentRejectPct}% rejected without a reason`}
            .
          </p>
        )}
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto px-1 pb-2">
        {entries.length === 0 ? (
          <SheetEmptyState>
            No decisions yet. Accept or reject a proposal from the Stage to
            start building history.
          </SheetEmptyState>
        ) : filtered.length === 0 ? (
          <SheetEmptyState>No decisions match these filters.</SheetEmptyState>
        ) : (
          <ol className="relative flex flex-col gap-2.5 pl-4">
            {/* connecting timeline hairline down the left edge */}
            <span
              className="pointer-events-none absolute bottom-2 left-[5px] top-2 w-px bg-[var(--glass-border)]"
              aria-hidden
            />
            {filtered.map((entry, idx) => (
              <li key={`${entry.id}-${entry.decidedAt}-${idx}`} className="relative">
                <SheetRow
                  entry={entry}
                  onRestore={() => onOpenRestore(entry)}
                />
              </li>
            ))}
          </ol>
        )}
      </div>

      {restoreEntry && (
        <RestoreDialog
          entry={restoreEntry}
          onClose={onCloseRestore}
          onRestored={onRestored}
        />
      )}

      {restoreToast && (
        <div
          role="status"
          aria-live="polite"
          className="glass-strong pointer-events-none absolute bottom-3 left-1/2 z-10 -translate-x-1/2 rounded-[12px] px-3 py-1.5 text-[11px] text-[var(--color-text-primary)]"
        >
          {restoreToast}
        </div>
      )}
    </section>
  );
}

function SheetRow({
  entry,
  onRestore,
}: {
  entry: HistoryEntry;
  onRestore: () => void;
}) {
  const medium = MEDIUM_STYLE[entry.medium];
  const badge = DECISION_BADGE[entry.decision];
  const accent = SHEET_DECISION_COLOR[entry.decision];
  const rationale =
    entry.decision === "rejected" && entry.rejectReason
      ? `Reason: ${truncateReason(entry.rejectReason)}`
      : entry.rationale;

  return (
    <div className="glass-content relative px-3.5 py-3">
      {/* timeline node — sits on the left hairline, colored by decision */}
      <span
        className="absolute -left-[15px] top-4 h-2 w-2 rounded-full ring-2 ring-[var(--glass-content)]"
        style={{ backgroundColor: accent }}
        aria-hidden
      />

      <div className="flex items-center gap-2">
        <span
          className={cn(
            "shrink-0 rounded-full border px-1.5 py-0.5 text-[10px] font-medium",
            badge.className,
          )}
        >
          {badge.label}
        </span>
        <span
          className={cn(
            "shrink-0 rounded-full border px-1.5 py-0.5 text-[10px] font-medium",
            medium.className,
          )}
        >
          {medium.label}
        </span>
        <span className="ml-auto shrink-0 font-mono text-[10px] text-[var(--color-text-muted)] tabular-nums">
          {formatRelative(entry.decidedAt)}
        </span>
        {canRestore(entry) && (
          <button
            type="button"
            onClick={onRestore}
            className="glass-ghost ml-1 inline-flex shrink-0 items-center gap-1 rounded-[10px] px-1.5 py-0.5 text-[10px]"
            title="Restore the timeline to this point"
          >
            <RotateCcw className="h-3 w-3" strokeWidth={1.75} />
            Restore
          </button>
        )}
      </div>

      <p className="mt-1.5 truncate text-[13px] font-medium text-[var(--color-text-primary)]">
        {entry.title}
      </p>
      {rationale && (
        <p className="mt-0.5 truncate text-[11px] text-[var(--color-text-muted)]">
          {rationale}
        </p>
      )}
    </div>
  );
}

function SheetChip({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        "glass-ghost h-6 rounded-full px-2.5 text-[10px] font-medium",
        active &&
          "border-[var(--color-brand)] text-[var(--color-text-primary)] glow-brand",
      )}
    >
      {label}
    </button>
  );
}

function SheetEmptyState({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex h-full w-full items-center justify-center p-8 text-center">
      <p className="max-w-xs text-[12px] leading-relaxed text-[var(--color-text-muted)]">
        {children}
      </p>
    </div>
  );
}

function filterByProject(all: HistoryEntry[], projectPath: string): HistoryEntry[] {
  // Already-newest-first thanks to `record(entry)` prepending; re-sort
  // here defensively in case a future migration changes insert order.
  return all
    .filter((e) => e.projectPath === projectPath)
    .sort((a, b) => b.decidedAt - a.decidedAt);
}

function applyFilters(
  entries: HistoryEntry[],
  opts: {
    decisionFilter: DecisionFilter;
    mediumFilter: MediumFilter;
    query: string;
  },
): HistoryEntry[] {
  const q = opts.query.trim().toLowerCase();
  return entries.filter((e) => {
    if (opts.decisionFilter !== "all" && e.decision !== opts.decisionFilter) {
      return false;
    }
    if (opts.mediumFilter !== "all" && e.medium !== opts.mediumFilter) {
      return false;
    }
    if (q.length > 0) {
      const hay = `${e.title} ${e.rationale ?? ""}`.toLowerCase();
      if (!hay.includes(q)) return false;
    }
    return true;
  });
}

type HistoryRowProps = {
  entry: HistoryEntry;
  onRestore: () => void;
};

function HistoryRow({ entry, onRestore }: HistoryRowProps) {
  const medium = MEDIUM_STYLE[entry.medium];
  const badge = DECISION_BADGE[entry.decision];

  return (
    <div
      className={cn(
        "flex items-center gap-3 border-b border-[var(--color-border-subtle)] px-3 py-2",
        "hover:bg-[var(--color-surface-hover)]",
      )}
    >
      <time
        className="w-24 shrink-0 font-mono text-[10px] text-[var(--color-text-muted)] tabular-nums"
        dateTime={new Date(entry.decidedAt).toISOString()}
        title={new Date(entry.decidedAt).toLocaleString()}
      >
        {formatRelative(entry.decidedAt)}
      </time>

      <span
        className={cn(
          "shrink-0 rounded-full border px-1.5 py-0.5 text-[10px] font-medium",
          medium.className,
        )}
      >
        {medium.label}
      </span>

      <SourcePill source={entry.source} />

      <div className="min-w-0 flex-1">
        <p className="truncate text-[12px] font-semibold text-[var(--color-text-primary)]">
          {entry.title}
        </p>
        {entry.rationale && (
          <p className="truncate text-[11px] italic text-[var(--color-text-muted)]">
            {entry.rationale}
          </p>
        )}
        {entry.decision === "rejected" && entry.rejectReason && (
          <p
            className="truncate text-[11px] italic text-[var(--color-text-muted)]"
            title={entry.rejectReason}
          >
            Reason: {truncateReason(entry.rejectReason)}
          </p>
        )}
      </div>

      <span
        className={cn(
          "shrink-0 rounded-full border px-1.5 py-0.5 text-[10px] font-medium",
          badge.className,
        )}
      >
        {badge.label}
      </span>

      {canRestore(entry) && (
        <button
          type="button"
          onClick={onRestore}
          className={cn(
            "shrink-0 inline-flex items-center gap-1 rounded-[var(--radius-sm)]",
            "border border-[var(--color-border-subtle)] px-1.5 py-0.5",
            "text-[10px] text-[var(--color-text-secondary)]",
            "hover:border-[var(--color-border-strong)] hover:text-[var(--color-text-primary)]",
          )}
          title="Restore the timeline to this point"
        >
          <RotateCcw className="h-3 w-3" strokeWidth={1.75} />
          Restore
        </button>
      )}
    </div>
  );
}

/**
 * Restore is available when we have an inline timeline snapshot.
 * Accepted decisions captured a snapshot; restored entries carry the
 * post-restore snapshot so the user can immediately undo the restore.
 * Reject/revise rows never carry a snapshot.
 */
function canRestore(entry: HistoryEntry): boolean {
  if (entry.decision !== "accepted" && entry.decision !== "restored") {
    return false;
  }
  return typeof entry.timelineSnapshot === "string" && entry.timelineSnapshot.length > 0;
}

/**
 * Source → "Agent" / "User edit" pill. All three current brief
 * sources (approval, proposed_edit, broll) are agent-emitted on the
 * wire today — the user-edit channel is reserved for Wave 4 when
 * direct-manipulation edits start landing in the propose stream. The
 * mapping is centralized so adding that case later is a one-line
 * change.
 */
function SourcePill({ source }: { source: BriefProposalSource }) {
  const isAgent = source === "approval" || source === "proposed_edit" || source === "broll";
  const label = isAgent ? "Agent" : "User edit";
  const Icon = isAgent ? Sparkles : User;
  return (
    <span
      className={cn(
        "shrink-0 inline-flex items-center gap-1 rounded-full",
        "border border-[var(--color-border-subtle)] bg-[var(--color-surface-app)]",
        "px-1.5 py-0.5 text-[10px] text-[var(--color-text-secondary)]",
      )}
    >
      <Icon className="h-3 w-3" strokeWidth={1.75} />
      {label}
    </span>
  );
}

function FilterChip({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        "h-6 rounded-full border px-2 text-[10px] font-medium transition-colors",
        active
          ? "border-[var(--color-brand)] bg-[var(--color-surface-selected)] text-[var(--color-text-primary)]"
          : "border-[var(--color-border-subtle)] bg-[var(--color-surface-app)] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]",
      )}
    >
      {label}
    </button>
  );
}

function EmptyState({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex h-full w-full items-center justify-center p-8 text-center">
      <p className="max-w-sm text-[12px] text-[var(--color-text-muted)]">
        {children}
      </p>
    </div>
  );
}

/**
 * Confirm-then-restore dialog. Restoring replaces the live OTIO with
 * the inline snapshot the History tab stored at decision time
 * (Wave 4 W4.5). The action is destructive in the sense that any
 * edits AFTER this entry are overwritten — we warn explicitly when
 * pending proposals exist because those will also vanish.
 *
 * Project-path guard: an entry from another project (a stale localStorage
 * blob, or the user opened a different project) is refused. The restore
 * is fundamentally tied to the OTIO at the time of capture and that
 * OTIO is only meaningful inside its originating project.
 */
function RestoreDialog({
  entry,
  onClose,
  onRestored,
}: {
  entry: HistoryEntry;
  onClose: () => void;
  onRestored: (toast: string) => void;
}) {
  const currentProject = useProjectStore((s) => s.current);
  const pendingCount = useBriefProposalsStore((s) => s.pending().length);
  const refreshTimeline = useTimelineStore((s) => s.refresh);
  const [isRestoring, setIsRestoring] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const projectMismatch =
    currentProject !== null && currentProject !== entry.projectPath;
  const hasSnapshot =
    typeof entry.timelineSnapshot === "string" &&
    entry.timelineSnapshot.length > 0;

  async function confirm() {
    if (!hasSnapshot || projectMismatch || !currentProject) return;
    setIsRestoring(true);
    setError(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("restore_timeline_otio", {
        snapshotJson: entry.timelineSnapshot,
      });
      // Capture the post-restore OTIO so the new history row's ↺ Restore
      // button can undo the restore. Best-effort — if it fails we still
      // record the event, just without a rollback target.
      let postRestoreSnapshot: string | undefined;
      try {
        const raw = await invoke<string>("read_timeline_otio_raw");
        postRestoreSnapshot = raw.trim().length > 0 ? raw : undefined;
      } catch {
        postRestoreSnapshot = undefined;
      }
      const restoreEntry = buildRestoreEntry({
        source: entry,
        projectPath: currentProject,
        postRestoreSnapshot,
      });
      useProposalHistoryStore.getState().record(restoreEntry);
      // Force the timeline store to pick up the new on-disk state.
      await refreshTimeline();
      onRestored(`Restored timeline to "${entry.title}"`);
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setIsRestoring(false);
    }
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Restore timeline"
      className="absolute inset-0 z-10 flex items-center justify-center bg-[rgba(0,0,0,0.55)] p-4"
      onClick={(e) => {
        if (e.target === e.currentTarget && !isRestoring) onClose();
      }}
    >
      <div className="panel max-w-md rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-app)] p-4">
        <div className="flex items-center gap-2 text-[var(--color-text-primary)]">
          <RotateCcw className="h-4 w-4" strokeWidth={1.75} />
          <h3 className="text-[13px] font-semibold">Restore this timeline state?</h3>
        </div>

        <p className="mt-3 text-[12px] leading-snug text-[var(--color-text-secondary)]">
          Replaces your current timeline with the snapshot captured when
          you accepted this proposal. The current timeline will be
          overwritten — any edits made since this point will be lost.
        </p>

        <p className="mt-3 rounded-[var(--radius-sm)] border-l-2 border-[var(--color-brand-secondary)] bg-[var(--color-surface-input)] px-2 py-1 font-mono text-[10px] text-[var(--color-text-muted)]">
          {entry.title} · {formatRelative(entry.decidedAt)}
        </p>

        {projectMismatch && (
          <p
            className={cn(
              "mt-3 rounded-[var(--radius-sm)] border-l-2",
              "border-[rgba(239,68,68,0.6)] bg-[rgba(239,68,68,0.08)]",
              "px-2 py-1 text-[11px] text-[#FCA5A5]",
            )}
          >
            This snapshot belongs to a different project
            (<code className="font-mono">{entry.projectPath}</code>). Open
            that project to restore.
          </p>
        )}

        {!hasSnapshot && !projectMismatch && (
          <p
            className={cn(
              "mt-3 rounded-[var(--radius-sm)] border-l-2",
              "border-[rgba(245,158,11,0.6)] bg-[rgba(245,158,11,0.08)]",
              "px-2 py-1 text-[11px] text-[#FCD34D]",
            )}
          >
            No snapshot is attached to this entry. This row predates the
            inline-snapshot system and can't be restored.
          </p>
        )}

        {pendingCount > 0 && !projectMismatch && hasSnapshot && (
          <p
            className={cn(
              "mt-3 rounded-[var(--radius-sm)] border-l-2",
              "border-[rgba(245,158,11,0.6)] bg-[rgba(245,158,11,0.08)]",
              "px-2 py-1 text-[11px] text-[#FCD34D]",
            )}
          >
            {pendingCount} pending proposal{pendingCount === 1 ? "" : "s"}{" "}
            on the Brief stack will become irrelevant after the restore —
            their ghost overlays reference the post-restore timeline.
          </p>
        )}

        {error && (
          <p
            className={cn(
              "mt-3 rounded-[var(--radius-sm)] border-l-2",
              "border-[rgba(239,68,68,0.6)] bg-[rgba(239,68,68,0.08)]",
              "px-2 py-1 text-[11px] text-[#FCA5A5]",
            )}
            role="alert"
          >
            Restore failed: {error}
          </p>
        )}

        <div className="mt-4 flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            disabled={isRestoring}
            className={cn(
              "h-7 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] px-3",
              "text-[11px] text-[var(--color-text-primary)]",
              "hover:border-[var(--color-border-strong)]",
              "disabled:opacity-60",
            )}
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={confirm}
            disabled={isRestoring || !hasSnapshot || projectMismatch}
            className={cn(
              "h-7 rounded-[var(--radius-sm)] px-3 text-[11px] font-medium",
              "border border-[var(--color-brand)]",
              "bg-[var(--color-brand)] text-[var(--color-text-on-accent)]",
              "hover:opacity-90 disabled:opacity-40 disabled:cursor-not-allowed",
            )}
          >
            {isRestoring ? "Restoring…" : "Restore"}
          </button>
        </div>
      </div>
    </div>
  );
}

/**
 * Soft-cap the reject reason on the history row so a long custom
 * reason doesn't blow up the row layout. The full reason still rides
 * on `title=` for the hover tooltip.
 */
function truncateReason(reason: string, max = 60): string {
  if (reason.length <= max) return reason;
  return `${reason.slice(0, max - 1)}…`;
}

/**
 * Relative timestamps tuned for history scanning: "just now",
 * "2m ago", "yesterday", and an absolute date for anything older
 * than a week. Exported for the test.
 */
export function formatRelative(ts: number, now: number = Date.now()): string {
  const delta = now - ts;
  if (delta < 0) return "just now";
  const seconds = Math.floor(delta / 1000);
  if (seconds < 30) return "just now";
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days === 1) return "yesterday";
  if (days < 7) return `${days}d ago`;
  return new Date(ts).toLocaleDateString();
}
