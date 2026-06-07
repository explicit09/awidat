// Zustand store for the in-flight EDL proposal. Subscribed to the
// `montage://item` Tauri channel for `Item::ProposedEdit` deltas.
// Drops Deltas with stale revisions (rapid-drag race protection).
//
// The backend's pending_proposals map is per-call_id, so concurrent
// proposals can coexist. The canvas still shows one ghost overlay at
// a time, but the store keeps the rest available for inspector
// selection instead of dropping them.

import { create } from "zustand";
import type {
  AppliedDiff,
  Item,
  ProposalAlternative,
  ProposalEvidence,
  ProposalSource,
  RiskLevel,
  TimelineSnapshot,
} from "../protocol";

/** Subset of `Item::ProposedEdit` the canvas + actions need. */
export type ActiveProposal = {
  callId: string;
  phase: "started" | "delta" | "completed";
  source: ProposalSource;
  edlText: string;
  snapshot: TimelineSnapshot;
  diffHints: AppliedDiff[];
  summary: string;
  revision: number;
  /** Inspector fields — all optional. Phase 2.8 added them to the protocol. */
  intent?: string;
  explanation?: string;
  confidence?: number;
  risk?: RiskLevel;
  evidence?: ProposalEvidence[];
  alternatives?: ProposalAlternative[];
  /**
   * Optional short-form rationale — the agent's one-sentence
   * justification ("trimmed 0.42s silence per podcast defaults").
   * Wave 3 renders this on every proposal pill / Brief row /
   * inspector header so the reviewer can take the call on faith.
   */
  rationale?: string;
};

type ProposalState = {
  /** The single in-flight proposal, or null when none is open. */
  active: ActiveProposal | null;
  /** All pending proposals known to the desktop, ordered by arrival. */
  pending: ActiveProposal[];
  /** Apply an Item::ProposedEdit emission. Drops stale Deltas. */
  ingest: (item: Extract<Item, { kind: "proposed_edit" }>) => void;
  /** Make one pending proposal the active canvas/inspector proposal. */
  select: (callId: string) => void;
  /** Drop the active proposal. Called after Accept/Reject completes. */
  clear: () => void;
};

export const useProposalStore = create<ProposalState>((set) => ({
  active: null,
  pending: [],
  ingest: (item) =>
    set((state) => {
      // Completed phase always wins — it ends the lifecycle.
      if (item.phase === "completed") {
        const pending = state.pending.filter(
          (proposal) => proposal.callId !== item.id,
        );
        if (state.active?.callId === item.id) {
          return { active: pending[pending.length - 1] ?? null, pending };
        }
        return { pending };
      }
      if (item.phase === "started") {
        const proposal = proposalFromStartedItem(item);
        return {
          active: proposal,
          pending: upsertProposal(state.pending, proposal),
        };
      }
      // Delta: only apply if the call_id matches and revision is
      // newer than what we have. Stale-Delta drops protect against
      // rapid-drag races.
      const pending = state.pending.map((proposal) =>
        proposal.callId === item.id
          ? proposalFromDeltaItem(proposal, item)
          : proposal,
      );
      if (state.active?.callId === item.id) {
        const active = proposalFromDeltaItem(state.active, item);
        return { active, pending: upsertProposal(pending, active) };
      }
      return { pending };
    }),
  select: (callId) =>
    set((state) => ({
      active:
        state.pending.find((proposal) => proposal.callId === callId) ??
        state.active,
    })),
  clear: () => set({ active: null, pending: [] }),
}));

function proposalFromStartedItem(
  item: Extract<Item, { kind: "proposed_edit" }>,
): ActiveProposal {
  return {
    callId: item.id,
    phase: "started",
    source: item.source,
    edlText: item.edl_text,
    snapshot: item.snapshot,
    diffHints: item.diff_hints,
    summary: item.summary,
    revision: item.revision,
    intent: item.intent ?? undefined,
    explanation: item.explanation ?? undefined,
    confidence: item.confidence ?? undefined,
    risk: item.risk ?? undefined,
    evidence: item.evidence ?? [],
    alternatives: item.alternatives ?? [],
    rationale: item.rationale ?? undefined,
  };
}

function proposalFromDeltaItem(
  existing: ActiveProposal,
  item: Extract<Item, { kind: "proposed_edit" }>,
): ActiveProposal {
  if (item.revision <= existing.revision) return existing;
  return {
    ...existing,
    phase: "delta",
    edlText: item.edl_text || existing.edlText,
    snapshot: item.snapshot,
    diffHints: item.diff_hints,
    summary: item.summary,
    revision: item.revision,
    intent: item.intent ?? existing.intent,
    explanation: item.explanation ?? existing.explanation,
    confidence: item.confidence ?? existing.confidence,
    risk: item.risk ?? existing.risk,
    evidence: item.evidence?.length ? item.evidence : existing.evidence,
    alternatives: item.alternatives?.length
      ? item.alternatives
      : existing.alternatives,
    rationale: item.rationale ?? existing.rationale,
  };
}

function upsertProposal(
  pending: readonly ActiveProposal[],
  proposal: ActiveProposal,
): ActiveProposal[] {
  const index = pending.findIndex((item) => item.callId === proposal.callId);
  if (index === -1) return [...pending, proposal];
  return pending.map((item, itemIndex) =>
    itemIndex === index ? proposal : item,
  );
}

export function isProposedEditItem(
  item: Item,
): item is Extract<Item, { kind: "proposed_edit" }> {
  return item.kind === "proposed_edit";
}
