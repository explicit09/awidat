// Focus proposal review on the shared preview and timeline.

import { create } from "zustand";
import type { AppliedDiff, TimelineSnapshot } from "../protocol";
import type { BriefMedium } from "./briefProposals";

/* ----------------------------- types ------------------------------- */

export interface FlashRange {
  /** Stable key — `${proposalId}:${idx}`. Used to clear individual
   *  flashes when they expire instead of nuking the whole set. */
  key: string;
  /** Optional track index when the flash is single-track; null means
   *  "all tracks at this time range" (used for transition / mixed). */
  trackIndex: number | null;
  /** Inclusive start in seconds along the *current* timeline. */
  startS: number;
  /** Exclusive end in seconds along the *current* timeline. */
  endS: number;
  /** Visual intent — emphasized when the proposal is a transition
   *  (subtler glow without the "fill" because the entity is thin). */
  kind: "clip" | "transition";
}

interface FlashRangesState {
  ranges: FlashRange[];
  /** Adds a flash and schedules its removal after `durationMs`. */
  add: (range: FlashRange, durationMs?: number) => void;
  clear: () => void;
}

const FLASH_DURATION_MS = 600;

export const useFlashRanges = create<FlashRangesState>((set, get) => ({
  ranges: [],
  add(range, durationMs = FLASH_DURATION_MS) {
    set((state) => ({
      ranges: [...state.ranges.filter((r) => r.key !== range.key), range],
    }));
    if (typeof setTimeout === "function") {
      setTimeout(() => {
        const next = get().ranges.filter((r) => r.key !== range.key);
        if (next.length !== get().ranges.length) {
          set({ ranges: next });
        }
      }, durationMs);
    }
  },
  clear() {
    set({ ranges: [] });
  },
}));

/* --------------------------- orchestrator -------------------------- */

/**
 * Side-effect adapter so the controller stays pure-ish (and testable).
 * Defaults wire to the real stores; tests can swap any field.
 */
export interface FocusAdapter {
  /** Drive the timeline-time playhead. */
  requestTimelineSeek: (t: number) => void;
  /** Best-effort horizontal scroll on the timeline stage so the
   *  given time range is centered in the viewport. Returns silently
   *  when the stage isn't mounted. Time → x conversion is left to the
   *  callee because pps lives on the canvas; we hand it a span and
   *  let it find the DOM node. */
  scrollTimelineTo: (centerTimeS: number) => void;
  /** Read the timeline currently used for playback and clip flashes. */
  readTimelineSnapshot: () => TimelineSnapshot | null;
}

/** Time range derived from a proposal's diff hints. */
export interface ProposalRange {
  /** Track index in the current snapshot, or null for "any track". */
  trackIndex: number | null;
  startS: number;
  endS: number;
  /** Exact edit point in current playback time, when the hint provides one. */
  reviewTimeS?: number;
}

/** Resolve review ranges in the current timeline, which owns playback.
 * Proposed item indices must never be used as current timeline indices.
 * New clips without a current identity have no playback range yet.
 */
export function deriveRanges(
  diffHints: ReadonlyArray<AppliedDiff>,
  currentSnapshot: TimelineSnapshot | null,
  proposedSnapshot: TimelineSnapshot | null,
): ProposalRange[] {
  const out: ProposalRange[] = [];
  for (const hint of diffHints) {
    const range = rangeForHint(hint, currentSnapshot, proposedSnapshot);
    if (range) out.push(range);
  }
  return out;
}

function rangeForHint(
  hint: AppliedDiff,
  currentSnapshot: TimelineSnapshot | null,
  proposedSnapshot: TimelineSnapshot | null,
): ProposalRange | null {
  if (!currentSnapshot) return null;
  if (hint.kind === "delete" || hint.kind === "move") {
    const trackIndex = hint.kind === "move" ? hint.from_track_index : hint.track_index;
    const itemIndex = hint.kind === "move" ? hint.from_item_index : hint.item_index;
    const item = currentSnapshot.tracks[trackIndex]?.items.find((it) => it.index === itemIndex);
    return item ? itemRange(item, trackIndex) : null;
  }

  const proposedItem = proposedSnapshot?.tracks[hint.track_index]?.items.find(
    (it) => it.index === hint.item_index,
  );
  if (proposedItem?.kind !== "clip" || !proposedItem.clip_uuid) return null;
  for (const [trackIndex, track] of currentSnapshot.tracks.entries()) {
    const item = track.items.find(
      (it) => it.kind === "clip" && it.clip_uuid === proposedItem.clip_uuid,
    );
    if (item?.kind === "clip") {
      const range = itemRange(item, trackIndex);
      if (hint.kind === "split" || hint.kind === "trim_edge") {
        const speed = item.speed && item.speed > 0 ? item.speed : 1;
        const proposedSpeed = proposedItem.speed && proposedItem.speed > 0 ? proposedItem.speed : 1;
        const sourceTime = hint.kind === "split" ? hint.at_s
          : (proposedItem.source_start_s ?? 0) +
            (hint.side === "right" ? proposedItem.duration_s * proposedSpeed : 0);
        range.reviewTimeS = Math.max(range.startS, Math.min(range.endS,
          item.track_start_s + (sourceTime - (item.source_start_s ?? 0)) / speed));
      }
      return range;
    }
  }
  return null;
}

function itemRange(
  item: TimelineSnapshot["tracks"][number]["items"][number],
  trackIndex: number,
): ProposalRange {
  return {
    trackIndex,
    startS: item.track_start_s,
    endS: item.track_start_s + Math.max(0.05, item.duration_s),
  };
}

/**
 * Union the ranges into a single covering span so the playhead has a
 * single time to seek to (the union midpoint). Returns null when
 * `ranges` is empty.
 */
export function unionRange(
  ranges: ReadonlyArray<ProposalRange>,
): { startS: number; endS: number } | null {
  if (ranges.length === 0) return null;
  let startS = Infinity;
  let endS = -Infinity;
  for (const r of ranges) {
    if (r.startS < startS) startS = r.startS;
    if (r.endS > endS) endS = r.endS;
  }
  return { startS, endS };
}

export interface FocusProposalArgs {
  proposalId: string;
  medium: BriefMedium;
  diffHints?: ReadonlyArray<AppliedDiff>;
  /** Snapshot the proposal projected for trim/insert/etc. hints. */
  proposedSnapshot?: TimelineSnapshot | null;
}

interface FocusControllerState {
  adapter: FocusAdapter;
  /** Central dispatch. See module header for the per-medium contract. */
  focusProposal: (args: FocusProposalArgs) => void;
}

const noopAdapter: FocusAdapter = {
  requestTimelineSeek: () => {},
  scrollTimelineTo: () => {},
  readTimelineSnapshot: () => null,
};

export const useFocusController = create<FocusControllerState>((_set, get) => ({
  adapter: noopAdapter,
  focusProposal(args) {
    const adapter = get().adapter;
    const { proposalId, medium, diffHints = [], proposedSnapshot = null } =
      args;
    const currentSnapshot = adapter.readTimelineSnapshot();

    // Resolve the proposal's covering time range (empty when no hints
    // resolve).
    const ranges = deriveRanges(diffHints, currentSnapshot, proposedSnapshot);
    const span = unionRange(ranges);

    if (["color", "broll", "title", "caption"].includes(medium)) {
      if (span) adapter.requestTimelineSeek(ranges[0]?.reviewTimeS ?? span.startS);
      return;
    }
    focusOnTimeline(adapter, proposalId, ranges, span,
      medium === "transition" ? "transition" : "clip");
  },
}));

/** Per-medium helper for the timeline branch — applies seek, scroll,
 *  and the flash glow for every resolved range. */
function focusOnTimeline(
  adapter: FocusAdapter,
  proposalId: string,
  ranges: ReadonlyArray<ProposalRange>,
  span: { startS: number; endS: number } | null,
  flashKind: "clip" | "transition",
): void {
  if (span) {
    const time = ranges[0]?.reviewTimeS ?? (span.startS + span.endS) / 2;
    adapter.requestTimelineSeek(time);
    adapter.scrollTimelineTo(time);
  }
  if (ranges.length === 0) {
    // No diff_hints we could resolve — best-effort full-track flash
    // skipped because there is no affected time range.
    return;
  }
  ranges.forEach((range, idx) => {
    useFlashRanges.getState().add({
      key: `${proposalId}:${idx}`,
      trackIndex: range.trackIndex,
      startS: range.startS,
      endS: range.endS,
      kind: flashKind,
    });
  });
}

/* --------------------------- defaults ------------------------------ */

/**
 * Wire the controller to the real frontend stores. App.tsx calls this
 * once on boot; tests skip it and provide their own adapter via
 * `useFocusController.setState`.
 */
export function installDefaultAdapter(adapter: FocusAdapter): void {
  useFocusController.setState({ adapter });
}
