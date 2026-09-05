// One proposal queue owns the timeline, inspector, and Brief lifecycle.
import { create } from "zustand";
import type {
  AppliedDiff, Item, ProposalAlternative, ProposalEvidence, ProposalSource,
  RiskLevel, TimelineSnapshot,
} from "../protocol";
import { deriveMedium, type ProposalMedium } from "./proposalMedium.ts";

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

export interface PendingProposal extends ActiveProposal {
  medium: ProposalMedium;
  firstSeenAt: number;
}

type ProposalState = {
  active: PendingProposal | null;
  pending: PendingProposal[];
  ingest: (item: Extract<Item, { kind: "proposed_edit" }>) => void;
  select: (callId: string) => void;
  clear: () => void;
};

export const useProposalStore = create<ProposalState>((set) => ({
  active: null,
  pending: [],
  ingest: (item) => set((state) => {
    if (item.phase === "completed") {
      const pending = state.pending.filter((proposal) => proposal.callId !== item.id);
      return {
        pending,
        active: state.active?.callId === item.id
          ? pending[pending.length - 1] ?? null : state.active,
      };
    }
    const existing = state.pending.find((proposal) => proposal.callId === item.id);
    // Late deltas cannot recreate completed proposals or rewind their revision.
    if (item.phase === "delta" && !existing) return state;
    if (existing && item.revision <= existing.revision) return state;
    const projected = existing
      ? proposalFromDeltaItem(existing, item) : proposalFromStartedItem(item);
    const proposal: PendingProposal = {
      ...projected,
      medium: deriveMedium(projected),
      firstSeenAt: existing?.firstSeenAt ?? Date.now(),
    };
    if (existing) recordRevision(existing);
    const pending = existing
      ? state.pending.map((entry) => entry.callId === item.id ? proposal : entry)
      : [...state.pending, proposal];
    return {
      pending,
      active: !existing || state.active?.callId === item.id ? proposal : state.active,
    };
  }),
  select: (callId) => set((state) => ({
    active: state.pending.find((proposal) => proposal.callId === callId) ?? state.active,
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

export function isProposedEditItem(
  item: Item,
): item is Extract<Item, { kind: "proposed_edit" }> {
  return item.kind === "proposed_edit";
}

/**
 * Append a "revised" event to the persisted history log for the prior
 * state of a pending proposal. Dynamic-imported to keep this module
 * free of the project-store / history-store dependency cycle (the
 * brief proposals store also imports here).
 */
function recordRevision(prior: PendingProposal): void {
  void Promise.all([import("../app/state"), import("../state/proposalHistory")])
    .then(([{ useProjectStore }, { buildHistoryEntry, useProposalHistoryStore }]) => {
      const projectPath = useProjectStore.getState().current;
      if (!projectPath) return;
      const entry = buildHistoryEntry({
        proposal: {
          id: prior.callId,
          source: "proposed_edit",
          medium: prior.medium,
          title:
            prior.summary && prior.summary.trim().length > 0
              ? prior.summary
              : "Proposed edit",
          rationale: prior.rationale,
          firstSeenAt: prior.firstSeenAt,
          toolName: undefined,
        },
        projectPath,
        decision: "revised",
      });
      useProposalHistoryStore.getState().record(entry);
    })
    .catch(() => {
      // History logging is best-effort chrome. Never fail an ingest.
    });
}
