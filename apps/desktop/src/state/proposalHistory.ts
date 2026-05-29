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
 *   - No size cap today. Proposal volume is low (humans review O(10s)
 *     per session) and the JSON round-trip is cheap. If the stream
 *     ever grows pathological we'd cap at e.g. 1k entries per project.
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

export type HistoryDecision = "accepted" | "rejected" | "revised";

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
 */
export function buildHistoryEntry(args: {
  proposal: BriefProposal;
  projectPath: string;
  decision: HistoryDecision;
  decidedAt?: number;
  turn_id?: string;
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
        r.decision === "revised")
    );
  });
}

export const useProposalHistoryStore = create<HistoryState>()(
  persist(
    (set, get) => ({
      entries: [],
      record: (entry) =>
        set((state) => ({ entries: [entry, ...state.entries] })),
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
      partialize: (state) => serialize(state.entries),
      merge: (persisted, current) => ({
        ...current,
        entries: deserialize(persisted),
      }),
    },
  ),
);
