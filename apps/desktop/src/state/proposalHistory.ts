/**
 * useProposalHistoryStore — the persisted log of proposal decisions.
 *
 * Why this exists (Wave 3 T4):
 *   Awidat's editorial flow is propose → ghost overlay → accept/reject/
 *   revise. The pending stack in `useBriefProposalsStore` is the LIVE
 *   view; once a decision lands the entry vanishes. This store keeps
 *   the audit trail — "git for video edits" — so the History tab can
 *   show every decision the editor (or the agent) has ever made on
 *   this project.
 *
 * Why not reconstruct from codex rollouts:
 *   Rollouts are turn-level. They lose proposal lifecycle nuance (a
 *   user-clicked Accept never appears as a turn) and replaying them is
 *   brittle across schema bumps. A dedicated log is simpler, has full
 *   control over event shape, persists across sessions via localStorage,
 *   and isolates this surface from backend churn.
 *
 * Persistence:
 *   - Zustand `persist` middleware → `awidat:proposal-history`.
 *   - Scoped per-project: entries carry `projectPath`; readers filter.
 *   - LRU-capped per project (Wave 4 W4.5): each accept stamps an inline
 *     OTIO snapshot for the History tab's ↺ Restore action. Snapshots
 *     can be tens of KB each, so we keep at most `MAX_ENTRIES_PER_PROJECT`
 *     entries per project — older entries fall out when the cap is hit.
 *     The cap is enforced on every record() call so a binge of accepts
 *     can't bloat localStorage past the browser's ~5 MB quota.
 *
 * Schema migration:
 *   The persisted shape uses a `version` field. Older shapes fall
 *   through `deserialize()` which returns `[]` for unknown versions —
 *   non-destructive (the user keeps using the app) but they lose
 *   history. When we bump the shape we add a `migrate` branch and
 *   bump the literal.
 */

import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";
// Type-only — avoids the import cycle between proposalHistory and
// briefProposals (briefProposals records history through us).
import type {
  BriefMedium,
  BriefProposal,
  BriefProposalSource,
  BrollDisclosureMetadata,
} from "./briefProposals";

export type HistoryDecision = "accepted" | "rejected" | "revised" | "restored";

/**
 * Max number of entries we retain per project. Each accepted entry
 * carries an inline OTIO snapshot (timeline.json string); typical
 * snapshots run a few KB but can hit ~50 KB on busy timelines. 100
 * entries × ~10 KB = ~1 MB worst case, comfortably under the browser's
 * ~5 MB localStorage budget shared across all Awidat state.
 *
 * Exported so tests can exercise eviction without redefining the cap.
 */
export const MAX_ENTRIES_PER_PROJECT = 100;

/**
 * One row in the History tab. The shape mirrors `BriefProposal` plus
 * the decision metadata. `projectPath` is non-optional so per-project
 * filtering can't accidentally leak entries across projects.
 */
export interface HistoryEntry {
  /** Same id as the originating BriefProposal (call_id / proposed_edit
   *  id / broll job id). Not unique across decisions — a single proposal
   *  can revise multiple times before final accept/reject. */
  id: string;
  /** Optional turn correlation — pulled from the active chat turn when
   *  available so future "open in chat" affordances can resolve back. */
  turn_id?: string;
  projectPath: string;
  medium: BriefMedium;
  source: BriefProposalSource;
  title: string;
  rationale?: string;
  toolName?: string;
  proposedAt: number;
  decision: HistoryDecision;
  decidedAt: number;
  /** Only set for generated-broll rows. */
  brollMetadata?: BrollDisclosureMetadata;
  /**
   * Raw `project.otio.json` text captured at the moment this decision
   * landed. Populated by `accept()` only — reject/revise paths leave it
   * undefined (no rollback target). Used by the History tab's ↺ Restore
   * action to replay the literal timeline shape at that point.
   *
   * Persisted inline in localStorage (Wave 4 W4.5 design choice — see
   * the file header comment). Bounded by `MAX_ENTRIES_PER_PROJECT`.
   */
  timelineSnapshot?: string;
}

/** Persisted shape — versioned for forward migration. */
type PersistedShape = {
  version: 1;
  entries: HistoryEntry[];
};

const STORAGE_KEY = "awidat:proposal-history";

interface HistoryState {
  /** Flat log, newest first. Bounded only by user volume. */
  entries: HistoryEntry[];
  /** Append a decision. No de-duplication; each Accept/Reject/Revise
   *  is a distinct event in the log. */
  record: (entry: HistoryEntry) => void;
  /** All entries for a given project, newest first. */
  forProject: (projectPath: string) => HistoryEntry[];
  /** Wipe a single project's history (used by tests + future "clear
   *  history" affordance in Settings → Project). */
  clearProject: (projectPath: string) => void;
  /** Drop everything (tests). */
  clear: () => void;
}

/**
 * Pure helper: build a HistoryEntry from a BriefProposal + decision
 * context. Exported so callers don't have to repeat the field copy
 * and so tests can exercise the shape without a store.
 *
 * `timelineSnapshot` is the raw `project.otio.json` text the caller
 * captured immediately before the dispatch landed. Pass undefined for
 * reject/revise paths or when no project is loaded.
 */
export function buildHistoryEntry(args: {
  proposal: BriefProposal;
  projectPath: string;
  decision: HistoryDecision;
  decidedAt?: number;
  turn_id?: string;
  timelineSnapshot?: string;
}): HistoryEntry {
  const { proposal, projectPath, decision } = args;
  const decidedAt = args.decidedAt ?? Date.now();
  return {
    id: proposal.id,
    turn_id: args.turn_id,
    projectPath,
    medium: proposal.medium,
    source: proposal.source,
    title: proposal.title,
    rationale: proposal.rationale,
    toolName: proposal.toolName,
    proposedAt: proposal.firstSeenAt,
    decision,
    decidedAt,
    brollMetadata: proposal.brollMetadata,
    timelineSnapshot: args.timelineSnapshot,
  };
}

/**
 * Pure helper: build a restore-event row. Restores are user-initiated
 * timeline rollbacks; logging them as their own decision so the audit
 * trail captures "user reverted to <title>" instead of pretending the
 * restored entry's accept happened twice.
 *
 * The new row stamps the *current* timeline (post-restore) so the user
 * can immediately undo the restore by clicking ↺ Restore on it. The
 * action is reversible by definition.
 */
export function buildRestoreEntry(args: {
  source: HistoryEntry;
  projectPath: string;
  postRestoreSnapshot?: string;
  decidedAt?: number;
}): HistoryEntry {
  const { source, projectPath } = args;
  const decidedAt = args.decidedAt ?? Date.now();
  return {
    id: `restore:${source.id}:${decidedAt}`,
    projectPath,
    medium: source.medium,
    source: source.source,
    title: `Restored: ${source.title}`,
    rationale: `Replayed timeline state from ${new Date(source.decidedAt).toLocaleString()}.`,
    toolName: undefined,
    proposedAt: source.decidedAt,
    decision: "restored",
    decidedAt,
    timelineSnapshot: args.postRestoreSnapshot,
  };
}

/** Newest-first sort. Stable for equal timestamps. */
export function sortNewestFirst(entries: HistoryEntry[]): HistoryEntry[] {
  return [...entries].sort((a, b) => b.decidedAt - a.decidedAt);
}

/** Project filter. Exported for tests. */
export function entriesForProject(
  all: HistoryEntry[],
  projectPath: string,
): HistoryEntry[] {
  return sortNewestFirst(all.filter((e) => e.projectPath === projectPath));
}

export function serialize(entries: HistoryEntry[]): PersistedShape {
  return { version: 1, entries };
}

export function deserialize(shape: unknown): HistoryEntry[] {
  if (!shape || typeof shape !== "object") return [];
  const obj = shape as { version?: unknown; entries?: unknown };
  if (obj.version !== 1) return [];
  if (!Array.isArray(obj.entries)) return [];
  // Defensive shape-check: drop entries missing the load-bearing keys.
  return obj.entries.filter((e: unknown): e is HistoryEntry => {
    if (!e || typeof e !== "object") return false;
    const r = e as Record<string, unknown>;
    return (
      typeof r.id === "string" &&
      typeof r.projectPath === "string" &&
      typeof r.title === "string" &&
      typeof r.medium === "string" &&
      typeof r.source === "string" &&
      typeof r.proposedAt === "number" &&
      typeof r.decidedAt === "number" &&
      (r.decision === "accepted" ||
        r.decision === "rejected" ||
        r.decision === "revised" ||
        r.decision === "restored")
    );
  });
}

/**
 * Apply the per-project LRU cap to a fresh entry list. Entries arrive
 * newest-first (record() prepends); we keep at most
 * `MAX_ENTRIES_PER_PROJECT` per `projectPath` and drop the oldest in
 * that bucket. Cross-project entries are untouched — caps are
 * per-project so a busy project doesn't evict entries from a different
 * project the user just opened.
 *
 * Exported so the eviction logic is unit-testable.
 */
export function enforcePerProjectCap(entries: HistoryEntry[]): HistoryEntry[] {
  // First pass: group counts by projectPath as we walk newest→oldest.
  // Second pass would re-scan; instead we collect indexes to drop in
  // one walk and filter at the end.
  const counts = new Map<string, number>();
  const drop = new Set<number>();
  for (let i = 0; i < entries.length; i++) {
    const project = entries[i].projectPath;
    const cur = counts.get(project) ?? 0;
    if (cur >= MAX_ENTRIES_PER_PROJECT) {
      drop.add(i);
    } else {
      counts.set(project, cur + 1);
    }
  }
  if (drop.size === 0) return entries;
  return entries.filter((_, i) => !drop.has(i));
}

export const useProposalHistoryStore = create<HistoryState>()(
  persist(
    (set, get) => ({
      entries: [],
      record: (entry) =>
        set((state) => ({
          entries: enforcePerProjectCap([entry, ...state.entries]),
        })),
      forProject: (projectPath) =>
        entriesForProject(get().entries, projectPath),
      clearProject: (projectPath) =>
        set((state) => ({
          entries: state.entries.filter((e) => e.projectPath !== projectPath),
        })),
      clear: () => set({ entries: [] }),
    }),
    {
      name: STORAGE_KEY,
      storage: createJSONStorage(() => localStorage),
      // Cap on persist too — defends against a state migration that
      // brings in a giant pre-cap history blob.
      partialize: (state) => serialize(enforcePerProjectCap(state.entries)),
      merge: (persisted, current) => ({
        ...current,
        entries: enforcePerProjectCap(deserialize(persisted)),
      }),
    },
  ),
);
