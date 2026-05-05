// Zustand store for the in-flight EDL proposal. Subscribed to the
// `awidat://item` Tauri channel for `Item::ProposedEdit` deltas.
// Drops Deltas with stale revisions (rapid-drag race protection).
//
// One active proposal at a time per AwidatState; if the agent
// emits a new one before the user accepts/rejects the old, the
// new one replaces the old in the store (the backend's
// pending_proposals map is per-call_id, so concurrent proposals
// from different sources can coexist there — but the canvas only
// ever shows one ghost overlay, so we keep the latest).

import { create } from "zustand";
import type {
  AppliedDiff,
  Item,
  ProposalSource,
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
};

type ProposalState = {
  /** The single in-flight proposal, or null when none is open. */
  active: ActiveProposal | null;
  /** Apply an Item::ProposedEdit emission. Drops stale Deltas. */
  ingest: (item: Extract<Item, { kind: "proposed_edit" }>) => void;
  /** Drop the active proposal. Called after Accept/Reject completes. */
  clear: () => void;
};

export const useProposalStore = create<ProposalState>((set) => ({
  active: null,
  ingest: (item) =>
    set((state) => {
      // Completed phase always wins — it ends the lifecycle.
      if (item.phase === "completed") {
        // If the call_id doesn't match the active one, ignore
        // (concurrent proposal cleanup; we keep the active one
        // until its own Completed lands).
        if (state.active?.callId === item.id) {
          return { active: null };
        }
        return state;
      }
      // Started always replaces — a new proposal can arrive even
      // while another is already showing (e.g. agent fires a second
      // apply_edl after the first was rejected).
      if (item.phase === "started") {
        return {
          active: {
            callId: item.id,
            phase: "started",
            source: item.source,
            edlText: item.edl_text,
            snapshot: item.snapshot,
            diffHints: item.diff_hints,
            summary: item.summary,
            revision: item.revision,
          },
        };
      }
      // Delta: only apply if the call_id matches and revision is
      // newer than what we have. Stale-Delta drops protect against
      // rapid-drag races.
      if (
        state.active?.callId === item.id &&
        item.revision > state.active.revision
      ) {
        return {
          active: {
            ...state.active,
            phase: "delta",
            edlText: item.edl_text || state.active.edlText,
            snapshot: item.snapshot,
            diffHints: item.diff_hints,
            summary: item.summary,
            revision: item.revision,
          },
        };
      }
      return state;
    }),
  clear: () => set({ active: null }),
}));

/** Subscribes the proposal store to ITEM_EVENT once at app start. */
export function isProposedEditItem(
  item: Item,
): item is Extract<Item, { kind: "proposed_edit" }> {
  return item.kind === "proposed_edit";
}
