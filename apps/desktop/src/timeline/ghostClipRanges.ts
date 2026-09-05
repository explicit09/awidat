// Derive timeline ghost-clip rectangles for cut-medium proposals
// (Wave 3 C2). Pure helper used by the renderer's ghost-overlay pass
// and by the DOM overlay that floats accept/reject buttons over each
// ghost.
//
// What "a ghost rectangle" means:
//   - { trackIndex, startS, endS } — a span on the *original* timeline
//     that the proposal touches. The reviewer sees this span outlined
//     in dashed cyan; ⌘↵ accepts it, Esc rejects it.
//
// Why "original timeline" and not the proposed snapshot?
//   - Many cut proposals (delete, ripple_delete) remove the clip
//     entirely — the *proposed* snapshot has no row to highlight.
//     Reviewers need to see "this is what's going away," which means
//     overlaying onto the current timeline they're looking at.
//   - For trim_edge/insert/move/split, the proposed snapshot is the
//     correct source — those clips exist there. We resolve from the
//     proposed snapshot when present; original snapshot is the
//     fallback for delete.
//
// Best-effort, not strict:
//   - If a diff hint points to an item index we can't find (race
//     between snapshot refresh and proposal ingest), we skip that hint
//     rather than crashing.
//   - If the proposal has no diff hints we can map to a range, we omit
//     the ghost entirely. The Brief stack remains the review surface
//     for that proposal — we don't claim to render and silently fail.

import type { AppliedDiff } from "../protocol";
import type { PendingProposal } from "./proposal.ts";
import type { TimelineItem, TimelineSnapshot } from "./store.ts";

export interface GhostRange {
  /** Proposal that owns this ghost. */
  proposalId: string;
  /** Track row in the *current* timeline (the one being rendered). */
  trackIndex: number;
  /** Start position on the track timeline, in seconds. */
  startS: number;
  /** End position on the track timeline, in seconds. */
  endS: number;
}

/**
 * Build the list of ghost rectangles for every cut-medium proposal.
 * Returns one entry per (proposal, affected region) — a single
 * proposal can produce multiple ghosts (e.g. a delete-multi op).
 */
export function buildGhostRanges(
  proposals: ReadonlyArray<PendingProposal>,
  currentSnapshot: TimelineSnapshot,
): GhostRange[] {
  const out: GhostRange[] = [];
  for (const proposal of proposals) {
    if (proposal.medium !== "cut") continue;
    const ranges = rangesForProposal(proposal, currentSnapshot);
    out.push(...ranges);
  }
  return out;
}

/**
 * Map a cut-medium proposal's diff hints to ranges on the current
 * timeline. Walks every hint; each hint can contribute zero or one
 * range. Hints we can't resolve (missing item, unknown kind) are
 * silently skipped.
 */
function rangesForProposal(
  proposal: PendingProposal,
  currentSnapshot: TimelineSnapshot,
): GhostRange[] {
  const ranges: GhostRange[] = [];
  for (const hint of proposal.diffHints) {
    const range = rangeForHint(hint, proposal, currentSnapshot);
    if (range) {
      ranges.push({ proposalId: proposal.callId, ...range });
    }
  }
  return ranges;
}

interface RawRange {
  trackIndex: number;
  startS: number;
  endS: number;
}

function rangeForHint(
  hint: AppliedDiff,
  proposal: PendingProposal,
  currentSnapshot: TimelineSnapshot,
): RawRange | null {
  switch (hint.kind) {
    case "delete": {
      // Delete hint indexes the original snapshot. Reviewer sees the
      // clip that's being removed.
      return lookupClip(currentSnapshot, hint.track_index, hint.item_index);
    }
    case "trim_edge":
    case "insert":
    case "insert_b_roll":
    case "insert_pi_p":
    case "split": {
      // These hint kinds index the *proposed* snapshot. Map back onto
      // the current timeline by track index — items shift; best we can
      // do without re-running an EDL diff is "highlight the same x
      // span on the same track" by reading the post-state clip's
      // bounds and rendering them at those bounds. The current
      // timeline doesn't have an item at that index yet (insert) or
      // at the new edge position (trim), but the span itself maps 1:1.
      const proposed = lookupClip(
        proposal.snapshot,
        hint.track_index,
        hint.item_index,
      );
      return proposed;
    }
    case "move": {
      // Use the destination (where the clip ends up). Reviewer sees
      // where it landed; the source position is implied by the
      // adjacent gap.
      return lookupClip(
        proposal.snapshot,
        hint.to_track_index,
        hint.to_item_index,
      );
    }
    default:
      return null;
  }
}

function lookupClip(
  snapshot: TimelineSnapshot,
  trackIndex: number,
  itemIndex: number,
): RawRange | null {
  const track = snapshot.tracks[trackIndex];
  if (!track) return null;
  const item: TimelineItem | undefined = track.items.find(
    (candidate) => candidate.index === itemIndex,
  );
  if (!item) return null;
  // Clamp positive duration so a zero-length ghost still draws.
  const duration = Math.max(0.05, item.duration_s);
  return {
    trackIndex,
    startS: item.track_start_s,
    endS: item.track_start_s + duration,
  };
}

/**
 * Compact ghost-range bucket keyed by proposal id, used by the DOM
 * overlay to position one button group per proposal (anchoring on the
 * first range — multi-range proposals get one shared hover overlay
 * rather than N duplicated ones).
 */
export interface GhostAnchor {
  proposalId: string;
  /** Track row to anchor the floating button-bar above. */
  trackIndex: number;
  /** Center-x of the ghost span, in seconds. */
  centerS: number;
  /** Start of the leftmost range, in seconds. Used to position
   *  tooltip / cheatsheet on the left edge. */
  startS: number;
  /** End of the rightmost range, in seconds. */
  endS: number;
}

/** Reduce ranges to one anchor per proposal — the union of its
 *  ranges. The button-bar floats above the union center. */
export function collapseToAnchors(
  ranges: ReadonlyArray<GhostRange>,
): GhostAnchor[] {
  const map = new Map<string, GhostAnchor>();
  for (const range of ranges) {
    const existing = map.get(range.proposalId);
    if (!existing) {
      map.set(range.proposalId, {
        proposalId: range.proposalId,
        trackIndex: range.trackIndex,
        startS: range.startS,
        endS: range.endS,
        centerS: (range.startS + range.endS) / 2,
      });
    } else {
      const startS = Math.min(existing.startS, range.startS);
      const endS = Math.max(existing.endS, range.endS);
      // Pick the topmost track so the floating button-bar doesn't
      // collide with a lower lane.
      const trackIndex = Math.min(existing.trackIndex, range.trackIndex);
      map.set(range.proposalId, {
        proposalId: range.proposalId,
        trackIndex,
        startS,
        endS,
        centerS: (startS + endS) / 2,
      });
    }
  }
  return Array.from(map.values());
}
