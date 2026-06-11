// Segment-boundary handoff math for the double-buffered preview.
//
// Transcript-led editing produces a cut every few seconds, so the
// preview crosses segment boundaries constantly. Each crossing used
// to stall visibly ("little bumps") for two reasons:
//
//   1. The swap always runs a few ms AFTER the boundary (rAF tick +
//      React effect latency), so the freshly swapped-in slot — parked
//      exactly on the editor's chosen cut-in frame — showed a tiny
//      "drift" against the timeline clock and was force-seeked with a
//      0.02s threshold. That re-seek stalled the decoder and skipped
//      the first frames of every cut.
// The fix: tolerate the scheduling overshoot on entry instead of
// re-seeking — the parked frame IS the right frame. (A "rolling
// preroll" that pre-played the hidden slot was tried and reverted:
// it caused src-less play() errors and decode thrash at high rates.
// The remaining paused→play start latency at swaps is the accepted
// cost.) Pure data math here, no DOM — see SegmentedVideoView for
// the wiring.

/** Drift tolerated when a segment is first entered while playing.
 *  Covers rAF cadence + effect latency (8–50ms at 1×, double at 2×)
 *  without re-seeking the already-parked cut-in frame. Larger drifts
 *  (mis-parked preroll, cold slot) still fall through to a seek. */
export const ENTRY_DRIFT_TOLERANCE_S = 0.15;

/** Above this effective rate, playback switches to shuttle: the
 *  element is paused and frames are stepped via currentTime off the
 *  clock (the proxy is all-keyframe precisely so each step decodes a
 *  single frame). Shuttle is SILENT — the paused element produces no
 *  audio — so the threshold sits at 4×, not at the decoder's ~2×
 *  comfort ceiling: between 2× and 4× the editor wants to keep
 *  hearing speech, and continuous decode degrades gracefully there
 *  (clock follows the lagging video). Beyond 4× video collapses
 *  entirely and smooth silent stepping wins. */
export const SHUTTLE_MIN_RATE = 4;

/** Frame-step cadence in shuttle mode (~12.5fps) — fast enough to
 *  read motion, slow enough that each single-keyframe seek finishes
 *  comfortably before the next. */
export const SHUTTLE_STEP_MS = 80;

/** Whether an effective element rate (segment speed × master rate)
 *  plays via shuttle stepping instead of continuous decode. */
export function isShuttleRate(effectiveRate: number): boolean {
  return Number.isFinite(effectiveRate) && effectiveRate > SHUTTLE_MIN_RATE;
}

/** Drift threshold (seconds) for syncing a media element to the
 *  timeline clock.
 *
 *  - `precise` — external seek or paused inspection: the user expects
 *    frame accuracy, correct anything beyond 0.02s.
 *  - `entry` — just crossed into the segment while playing: tolerate
 *    scheduling overshoot, see ENTRY_DRIFT_TOLERANCE_S.
 *  - steady-state playback: only correct real runaway drift.
 */
export function syncDriftThresholdS(opts: {
  precise: boolean;
  entry: boolean;
  elementPaused: boolean;
}): number {
  if (opts.precise) return 0.02;
  if (opts.entry) return ENTRY_DRIFT_TOLERANCE_S;
  return opts.elementPaused ? 0.02 : 0.5;
}

/** Element lag beyond which drift no longer reads as a decode
 *  underrun but as a real discontinuity (wrong content loaded,
 *  stale slot) — recover by seeking the element, not the clock. */
export const UNDERRUN_REBASE_MAX_S = 2;

/** Element lag at which the clock starts following the media during
 *  playback. Tight on purpose: at high playback rates (>2×) decode
 *  underruns continuously, and a loose trigger lets the playhead run
 *  ahead and snap back in a visible sawtooth ("frames left behind").
 *  Re-basing the clock is free — no decoder impact — so tracking the
 *  media closely costs nothing. */
export const UNDERRUN_REBASE_TRIGGER_S = 0.15;

/** How playback should recover from drift between the timeline clock
 *  and the active media element. `driftS` is desired − actual element
 *  position (positive = the element is behind the clock).
 *
 *  - `"none"` — drift within tolerance for this context.
 *  - `"seekElement"` — real discontinuity (external seek, paused
 *    frame-stepping, element ahead, or hopeless lag): move the media.
 *  - `"rebaseClock"` — the element is behind because decode can't
 *    keep up (seek latency, 2× playback underrun). Yanking it forward
 *    re-stalls the decoder and cascades; let the playhead wait for
 *    the media instead.
 */
export function driftRecoveryAction(opts: {
  precise: boolean;
  entry: boolean;
  elementPaused: boolean;
  driftS: number;
}): "none" | "seekElement" | "rebaseClock" {
  if (!Number.isFinite(opts.driftS)) return "none";
  const threshold = syncDriftThresholdS(opts);
  // The rebase trigger is tighter than the steady-state seek
  // threshold so sustained underrun tracks the media smoothly, but
  // never tighter than the context's own tolerance (a paused element
  // during play-start latency rebases from its 0.02s threshold).
  const rebaseTrigger = Math.min(UNDERRUN_REBASE_TRIGGER_S, threshold);
  if (
    !opts.precise &&
    !opts.entry &&
    opts.driftS > rebaseTrigger &&
    opts.driftS < UNDERRUN_REBASE_MAX_S
  ) {
    return "rebaseClock";
  }
  if (Math.abs(opts.driftS) <= threshold) return "none";
  return "seekElement";
}

/** Map a media element's source position back to timeline time within
 *  its segment, clamped to the segment's own range so float noise at
 *  the edges can't escape into a neighboring segment. Used to re-base
 *  the preview clock onto the element after a seek or decode underrun
 *  completes — re-seeking the element against a clock that ran ahead
 *  re-stalls the decoder and cascades (seek → canplay → clock ran
 *  ahead → seek …). The playhead follows the media, not vice versa. */
export function timelineTimeForSegmentPosition(
  seg: {
    timelineStart: number;
    timelineEnd: number;
    sourceStart: number;
    speed: number;
  },
  sourceTimeS: number,
): number {
  const safeSpeed = Number.isFinite(seg.speed) && seg.speed > 0 ? seg.speed : 1;
  const mapped = Number.isFinite(sourceTimeS)
    ? seg.timelineStart + (sourceTimeS - seg.sourceStart) / safeSpeed
    : seg.timelineStart;
  const end = Math.max(seg.timelineStart, seg.timelineEnd - 0.001);
  return Math.min(end, Math.max(seg.timelineStart, mapped));
}
