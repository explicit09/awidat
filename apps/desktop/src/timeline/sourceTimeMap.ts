// Inverse of `sourceTimeForTimelineTime`: map a moment in SOURCE
// media (e.g. a detected silence at 612s into raw/foo.MOV) to its
// position on the master timeline. Returns null when that source
// range is not on the timeline (already cut out) — callers use this
// to drop stale detections instead of jumping nowhere.
//
// Pure data math (no React/stores) so it's unit-testable in node.

export type SourceMappableSegment = {
  /** Proxy stem (read_transcript / read_silences key). */
  proxyStem: string;
  /** Source asset stem, when known. */
  sourceStem: string | null;
  /** Start offset into the source media, in seconds. */
  sourceStart: number;
  /** End offset into the source media, in seconds. */
  sourceEnd: number;
  /** Where the segment begins on the master timeline, in seconds. */
  timelineStart: number;
  /** Effective playback speed (source seconds per timeline second). */
  speed: number;
};

export function timelineTimeForSourceTime(
  segments: readonly SourceMappableSegment[],
  stem: string,
  sourceTimeS: number,
): number | null {
  if (!Number.isFinite(sourceTimeS)) return null;
  for (const seg of segments) {
    if (seg.proxyStem !== stem && seg.sourceStem !== stem) continue;
    if (sourceTimeS < seg.sourceStart || sourceTimeS >= seg.sourceEnd) continue;
    const speed = Number.isFinite(seg.speed) && seg.speed > 0 ? seg.speed : 1;
    return seg.timelineStart + (sourceTimeS - seg.sourceStart) / speed;
  }
  return null;
}

/**
 * How much of the source span [startS, endS) is still on the
 * timeline, and where the surviving part begins. Edits that cut INTO
 * a detected moment (e.g. removing 1.2s out of a 2.3s silence) leave
 * its start mapped but shrink the real remainder — callers use the
 * overlap to re-threshold and re-label instead of reporting the
 * original duration. Returns null when nothing survives.
 */
export function onTimelineSpan(
  segments: readonly SourceMappableSegment[],
  stem: string,
  startS: number,
  endS: number,
): { overlapS: number; firstSourceS: number } | null {
  if (!Number.isFinite(startS) || !Number.isFinite(endS) || endS <= startS) {
    return null;
  }
  let overlapS = 0;
  let firstSourceS = Number.POSITIVE_INFINITY;
  for (const seg of segments) {
    if (seg.proxyStem !== stem && seg.sourceStem !== stem) continue;
    const lo = Math.max(startS, seg.sourceStart);
    const hi = Math.min(endS, seg.sourceEnd);
    if (hi <= lo) continue;
    overlapS += hi - lo;
    if (lo < firstSourceS) firstSourceS = lo;
  }
  return overlapS > 0 ? { overlapS, firstSourceS } : null;
}
