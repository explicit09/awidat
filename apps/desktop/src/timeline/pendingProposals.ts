// Zustand store accumulating every in-flight proposal into a stack the
// Brief surface (B1+B2) renders as "5 proposals waiting — 2 cuts, 1
// color, 2 captions". The existing `useProposalStore` keeps the single
// ACTIVE proposal for the ghost overlay; this store sits alongside it
// and tracks the full pending queue.
//
// Lifecycle: Started seeds the entry, Delta updates it in place,
// Completed (which fires for both Accept and Reject) removes it.
// `firstSeenAt` is preserved across Delta updates so the Brief can
// surface "this has been waiting 2m" badges.
//
// Medium derivation walks the proposal's `edl_text` because the
// projected `ActiveProposal` carries the EDL as a string (the only
// op-level structure on the wire is `diff_hints`, which doesn't cover
// color/audio/transition/title kinds). We scan for `*** <Heading>`
// markers — the same headings `edlBuilder.ts` emits.

import { create } from "zustand";
import type {
  AppliedDiff,
  Item,
  ProposalAlternative,
  ProposalEvidence,
  ProposalSource,
  TimelineSnapshot,
} from "../protocol";
import type { ActiveProposal } from "./proposal";

export type ProposalMedium =
  | "cut" // delete/trim/split/ripple/move — review on timeline
  | "color" // set_color_correction / apply_lut — review on preview
  | "audio" // volume / fade / lead / trail / fx / ducking — review on waveform
  | "transition" // insert/delete_transition — review on timeline
  | "broll" // insert_b_roll diff hint — review on preview + insertion
  | "title" // insert_title / set_title — review on preview
  | "caption" // insert_caption / set_caption / caption_* — review in transcript
  | "mixed" // proposal touches >1 medium — Brief decides
  | "other"; // anything we didn't classify (animations, track tail, etc.)

export interface PendingProposal extends ActiveProposal {
  /** Derived from inspecting the proposal's ops/diff_hints. */
  medium: ProposalMedium;
  /** When this proposal first entered the pending list (ms epoch). */
  firstSeenAt: number;
}

type ProposedEditItem = Extract<Item, { kind: "proposed_edit" }>;

interface PendingState {
  /** Ordered by firstSeenAt ascending (oldest first). */
  pending: PendingProposal[];
  /** Apply an Item::ProposedEdit emission. Mirrors useProposalStore. */
  ingest(item: ProposedEditItem): void;
  /** Drop a proposal by id (e.g. after accept/reject completes). */
  remove(id: string): void;
  /** Drop every pending entry (project close / reset). */
  clear(): void;
  /** Group pending entries by their derived medium. */
  byMedium(): Record<ProposalMedium, PendingProposal[]>;
}

export const usePendingProposals = create<PendingState>((set, get) => ({
  pending: [],
  ingest(item) {
    set((state) => {
      // Completed fires for both Accept and Reject — it always ends
      // the lifecycle. Drop the entry whether or not we had it.
      if (item.phase === "completed") {
        const next = state.pending.filter((p) => p.callId !== item.id);
        return next.length === state.pending.length ? state : { pending: next };
      }

      const existing = state.pending.find((p) => p.callId === item.id);
      const projected = projectFromItem(item, existing);
      const firstSeenAt = existing?.firstSeenAt ?? Date.now();
      const medium = deriveMedium(projected);
      const entry: PendingProposal = { ...projected, medium, firstSeenAt };

      if (existing) {
        // Revision detection: when the agent re-emits a proposal with
        // a bumped revision counter we log the prior state as
        // "revised" so the History tab tells a complete story. Cheap
        // side-effect — the persisted log silently drops the entry
        // when no project is loaded.
        if (
          existing.revision !== undefined &&
          entry.revision !== undefined &&
          entry.revision > existing.revision
        ) {
          recordRevision(existing);
        }
        return {
          pending: state.pending.map((p) =>
            p.callId === item.id ? entry : p,
          ),
        };
      }
      return { pending: [...state.pending, entry] };
    });
  },
  remove(id) {
    set((state) => {
      const next = state.pending.filter((p) => p.callId !== id);
      return next.length === state.pending.length ? state : { pending: next };
    });
  },
  clear() {
    set({ pending: [] });
  },
  byMedium() {
    const out: Record<ProposalMedium, PendingProposal[]> = {
      cut: [],
      color: [],
      audio: [],
      transition: [],
      broll: [],
      title: [],
      caption: [],
      mixed: [],
      other: [],
    };
    for (const p of get().pending) {
      out[p.medium].push(p);
    }
    return out;
  },
}));

/**
 * Project an `Item::ProposedEdit` payload to the same `ActiveProposal`
 * shape useProposalStore exposes, preserving older field values on
 * Delta phases the way the active store does.
 */
function projectFromItem(
  item: ProposedEditItem,
  prev: PendingProposal | undefined,
): ActiveProposal {
  const isDelta = item.phase === "delta";
  const source: ProposalSource = item.source;
  const snapshot: TimelineSnapshot = item.snapshot;
  const diffHints: AppliedDiff[] = item.diff_hints;
  const evidence: ProposalEvidence[] = item.evidence ?? [];
  const alternatives: ProposalAlternative[] = item.alternatives ?? [];

  return {
    callId: item.id,
    phase: item.phase,
    source,
    edlText: isDelta && !item.edl_text ? (prev?.edlText ?? "") : item.edl_text,
    snapshot,
    diffHints,
    summary: item.summary,
    revision: item.revision,
    intent: item.intent ?? prev?.intent,
    explanation: item.explanation ?? prev?.explanation,
    confidence: item.confidence ?? prev?.confidence,
    risk: item.risk ?? prev?.risk,
    evidence: evidence.length ? evidence : (prev?.evidence ?? []),
    alternatives: alternatives.length ? alternatives : (prev?.alternatives ?? []),
    // Rationale follows the same preserve-on-omit policy: drag-adjust
    // Deltas typically omit the agent's rationale, but the Brief row
    // and pill tooltip need it across the full pending lifecycle.
    rationale: item.rationale ?? prev?.rationale,
  };
}

/**
 * Mediums each op heading maps to. These are the exact `***` headings
 * emitted by `edlBuilder.ts::appendOp` plus the agent-side variants
 * the backend produces. Anything not listed falls through to "other".
 */
const HEADING_MEDIUM: Record<string, ProposalMedium> = {
  // Cut family — anything that moves, removes, or splits clips.
  "Trim Clip": "cut",
  "Delete Clip": "cut",
  "Split Clip": "cut",
  "Move Clip": "cut",
  "Ripple Move": "cut",
  "Ripple Delete": "cut",
  "Ripple Trim": "cut",
  "Delete Gap": "cut",
  "Trim Track Tail": "cut",
  "Delete Track": "cut",
  "Professional Timeline Edit": "cut",
  "Set Speed": "cut",
  // Color family.
  "Set Color Correction": "color",
  "Apply LUT": "color",
  "Remove LUT": "color",
  // Audio family — everything touching levels, fades, fx, ducking.
  "Set Volume": "audio",
  "Set Audio Fade": "audio",
  "Set Audio Lead": "audio",
  "Set Audio Trail": "audio",
  "Set Track Audio": "audio",
  "Set Ducking": "audio",
  "Set Sync Group": "audio",
  "Set Clip Audio FX": "audio",
  "Set Track Audio FX": "audio",
  // Transition family.
  "Insert Transition": "transition",
  "Delete Transition": "transition",
  "Set Cut Intent": "transition",
  // Title family.
  "Insert Title": "title",
  "Set Title": "title",
  // Caption family — its own medium since captions live on the
  // transcript track, not the title overlay surface.
  "Insert Caption": "caption",
  "Set Caption": "caption",
};

/**
 * Walk the proposal's EDL text + diff hints and decide which medium
 * the proposal belongs to. Returns "mixed" when the proposal spans
 * more than one medium, "other" when nothing classifiable is found.
 */
export function deriveMedium(proposal: ActiveProposal): ProposalMedium {
  const mediums = new Set<ProposalMedium>();

  // 1. Scan `*** <Heading>` lines in the EDL text. This catches the
  //    full set in `HEADING_MEDIUM` above — including color/audio/
  //    transition/title which the diff_hints stream doesn't expose.
  const headingRe = /^\*\*\* (?!Begin EDL|End EDL)(.+?)$/gm;
  for (const match of proposal.edlText.matchAll(headingRe)) {
    const medium = HEADING_MEDIUM[match[1].trim()];
    if (medium) mediums.add(medium);
  }

  // 2. Diff hints catch the b-roll case the headings can't — b-roll
  //    arrives as a normal `Insert Clip` heading but the backend tags
  //    it via the `insert_b_roll` hint kind. Picture-in-picture is
  //    classed as b-roll too (same review surface — preview overlay).
  for (const hint of proposal.diffHints) {
    if (hint.kind === "insert_b_roll" || hint.kind === "insert_pi_p") {
      mediums.add("broll");
    }
  }

  if (mediums.size === 0) return "other";
  if (mediums.size === 1) return mediums.values().next().value as ProposalMedium;
  return "mixed";
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
