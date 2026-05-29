// Focus controller (Wave 4 W4.6) — turns a Brief proposal's "Review →"
// into a real focus action on the relevant surface.
//
// The Brief stack used to stop at the tab switch — Cut went to Timeline,
// Color/B-roll/Title to Source — leaving the reviewer to hunt down the
// affected time range or transcript segment by hand. This module wires
// the full motion:
//
//   * Switch the center tab to the right surface.
//   * Seek the playhead to the proposal's primary time.
//   * Scroll the timeline / transcript so the entity is in view.
//   * Flash the entity briefly so the eye lands on it.
//
// Architecture: one orchestrator (`useFocusController.focusProposal`) +
// three tiny sub-stores the surfaces subscribe to:
//
//   * `useFlashRanges`  — Set of timeline time-ranges flashed for ~600ms;
//     the timeline canvas paints a glow on items intersecting them.
//   * `useSourceFocus`  — Sub-tab + "coming soon" toast for Source view,
//     subscribed by `<TranscriptSource>`. Also drives the transcript
//     word-flash via the same store (separate sub-tab? same animation
//     budget — we keep it together).
//
// Why a separate store from `useCenterModeStore`: the center-mode store
// is pure persistence (per-project chosen tab). The focus controller is
// an ephemeral dispatch. Conflating them would either persist focus
// flashes (wrong) or strip persistence from tab choice (worse).
//
// Best-effort, never-throws: every sub-action is guarded so a missing
// snapshot, an empty diff_hints list, or a project without a transcript
// stem all degrade gracefully — worst case the user sees the tab switch
// without the flash, which is the Wave 3 contract.

import { create } from "zustand";
import type { AppliedDiff, TimelineSnapshot } from "../protocol";
import type { CenterMode } from "./centerMode";
import type { BriefMedium } from "./briefProposals";

/* ----------------------------- types ------------------------------- */

/** Optional source-tab focus when the Source pane is active. */
export type SourceSubTab = "transcript" | "video";

/** Discrete focus-toast kinds rendered in the Source pane. */
export type SourceToastKind = "before-after" | "insert-preview" | null;

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

/** Word-level transcript flash — drives the briefly-highlighted spans
 *  inside `<TranscriptSource>`. */
export interface TranscriptFlash {
  /** Stable key — `${proposalId}:${idx}`. */
  key: string;
  /** Inclusive source-time start in seconds. */
  startS: number;
  /** Exclusive source-time end in seconds. */
  endS: number;
}

interface FlashRangesState {
  ranges: FlashRange[];
  /** Adds a flash and schedules its removal after `durationMs`. */
  add: (range: FlashRange, durationMs?: number) => void;
  clear: () => void;
}

interface SourceFocusState {
  subTab: SourceSubTab;
  toast: SourceToastKind;
  /** Counter — TranscriptSource's effect re-runs when this bumps so a
   *  "switch to video, even if you're already on it" still re-flashes. */
  subTabRequestId: number;
  setSubTab: (next: SourceSubTab) => void;
  setToast: (next: SourceToastKind, durationMs?: number) => void;
  clear: () => void;
}

interface TranscriptFlashState {
  flashes: TranscriptFlash[];
  add: (flash: TranscriptFlash, durationMs?: number) => void;
  clear: () => void;
}

/* --------------------------- sub-stores ---------------------------- */

const FLASH_DURATION_MS = 600;
const TOAST_DURATION_MS = 2400;

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

export const useSourceFocus = create<SourceFocusState>((set) => ({
  subTab: "transcript",
  toast: null,
  subTabRequestId: 0,
  setSubTab(next) {
    set((state) => ({
      subTab: next,
      subTabRequestId: state.subTabRequestId + 1,
    }));
  },
  setToast(next, durationMs = TOAST_DURATION_MS) {
    set({ toast: next });
    if (next !== null && typeof window !== "undefined") {
      window.setTimeout(() => {
        // Only clear if the same toast is still showing; otherwise a
        // newer toast scheduled while we were waiting wins.
        if (useSourceFocus.getState().toast === next) {
          set({ toast: null });
        }
      }, durationMs);
    }
  },
  clear() {
    set({ subTab: "transcript", toast: null });
  },
}));

export const useTranscriptFlashes = create<TranscriptFlashState>((set, get) => ({
  flashes: [],
  add(flash, durationMs = FLASH_DURATION_MS) {
    set((state) => ({
      flashes: [
        ...state.flashes.filter((f) => f.key !== flash.key),
        flash,
      ],
    }));
    if (typeof setTimeout === "function") {
      setTimeout(() => {
        const next = get().flashes.filter((f) => f.key !== flash.key);
        if (next.length !== get().flashes.length) {
          set({ flashes: next });
        }
      }, durationMs);
    }
  },
  clear() {
    set({ flashes: [] });
  },
}));

/* --------------------------- orchestrator -------------------------- */

/**
 * Side-effect adapter so the controller stays pure-ish (and testable).
 * Defaults wire to the real stores; tests can swap any field.
 */
export interface FocusAdapter {
  /** Push the center pane to a new tab. */
  setCenterMode: (mode: CenterMode) => void;
  /** Drive the timeline-time playhead. */
  requestTimelineSeek: (t: number) => void;
  /** Best-effort horizontal scroll on the timeline stage so the
   *  given time range is centered in the viewport. Returns silently
   *  when the stage isn't mounted. Time → x conversion is left to the
   *  callee because pps lives on the canvas; we hand it a span and
   *  let it find the DOM node. */
  scrollTimelineTo: (centerTimeS: number) => void;
  /** Read the current snapshot — used to derive a "flash every clip"
   *  for mixed/other when no diff_hints carry time info. */
  readTimelineSnapshot: () => TimelineSnapshot | null;
}

/** Time range derived from a proposal's diff hints. */
export interface ProposalRange {
  /** Track index in the current snapshot, or null for "any track". */
  trackIndex: number | null;
  startS: number;
  endS: number;
}

/**
 * Walk a proposal's diff hints + optional snapshot and synthesize one
 * or more `ProposalRange` entries we can flash. The pendingProposals
 * store carries `snapshot` (the proposed snapshot) for trim/insert/
 * split/move kinds; delete hints index the original snapshot which
 * the caller passes in as `currentSnapshot`.
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
  switch (hint.kind) {
    case "delete": {
      // delete indexes the *original* timeline.
      const item = lookupItem(currentSnapshot, hint.track_index, hint.item_index);
      return item
        ? {
            trackIndex: hint.track_index,
            startS: item.start,
            endS: item.end,
          }
        : null;
    }
    case "trim_edge":
    case "insert":
    case "insert_b_roll":
    case "insert_pi_p":
    case "split": {
      const item = lookupItem(proposedSnapshot, hint.track_index, hint.item_index);
      return item
        ? {
            trackIndex: hint.track_index,
            startS: item.start,
            endS: item.end,
          }
        : null;
    }
    case "move": {
      const item = lookupItem(
        proposedSnapshot,
        hint.to_track_index,
        hint.to_item_index,
      );
      return item
        ? {
            trackIndex: hint.to_track_index,
            startS: item.start,
            endS: item.end,
          }
        : null;
    }
    default:
      return null;
  }
}

function lookupItem(
  snapshot: TimelineSnapshot | null,
  trackIndex: number,
  itemIndex: number,
): { start: number; end: number } | null {
  if (!snapshot) return null;
  const track = snapshot.tracks[trackIndex];
  if (!track) return null;
  const item = track.items.find((it) => it.index === itemIndex);
  if (!item) return null;
  const duration = Math.max(0.05, item.duration_s);
  return { start: item.track_start_s, end: item.track_start_s + duration };
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
  setCenterMode: () => {},
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
    // resolve — controller still switches tabs).
    const ranges = deriveRanges(diffHints, currentSnapshot, proposedSnapshot);
    const span = unionRange(ranges);

    switch (medium) {
      case "cut":
      case "audio":
      case "transition":
      case "mixed":
      case "other": {
        adapter.setCenterMode("timeline");
        focusOnTimeline(
          adapter,
          proposalId,
          ranges,
          span,
          medium === "transition" ? "transition" : "clip",
        );
        return;
      }
      case "color": {
        adapter.setCenterMode("source");
        useSourceFocus.getState().setSubTab("video");
        if (span) adapter.requestTimelineSeek(span.startS);
        useSourceFocus.getState().setToast("before-after");
        return;
      }
      case "broll": {
        adapter.setCenterMode("source");
        useSourceFocus.getState().setSubTab("video");
        if (span) adapter.requestTimelineSeek(span.startS);
        useSourceFocus.getState().setToast("insert-preview");
        return;
      }
      case "title": {
        adapter.setCenterMode("source");
        // Transcript-first projects default the sub-tab to transcript
        // so we leave it alone there; non-transcript projects keep
        // their last-picked sub-tab. Flash by source-time range when
        // we have one — the TranscriptSource subscriber locates the
        // matching `[data-word-start]` spans.
        if (span) {
          useTranscriptFlashes.getState().add({
            key: proposalId,
            startS: span.startS,
            endS: span.endS,
          });
        }
        return;
      }
      case "caption": {
        // Captions live on the transcript track — always force the
        // Transcript sub-tab so the reviewer sees the text the agent
        // is rewriting. Transcript-first projects already default
        // there; setting it explicitly makes the contract uniform.
        adapter.setCenterMode("source");
        useSourceFocus.getState().setSubTab("transcript");
        if (span) {
          useTranscriptFlashes.getState().add({
            key: proposalId,
            startS: span.startS,
            endS: span.endS,
          });
        }
        return;
      }
    }
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
    const midpoint = (span.startS + span.endS) / 2;
    adapter.requestTimelineSeek(midpoint);
    adapter.scrollTimelineTo(midpoint);
  }
  if (ranges.length === 0) {
    // No diff_hints we could resolve — best-effort full-track flash
    // skipped (it would be noisy). The seek + tab switch are enough.
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
