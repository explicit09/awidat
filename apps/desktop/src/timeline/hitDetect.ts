// Pointer hit-detection helpers for the TimelinePane canvas.
//
// Given a pointer position in canvas-local coordinates, figure out
// whether the user is hovering near a clip edge. Drives both the
// cursor hint (`cursor: ew-resize` near edges) and the drag-to-trim
// pointer-down branch.

import type { TimelineSnapshot, TimelineItem } from "./store";

/** Pointer-pixel tolerance for "near a clip edge". */
export const EDGE_HIT_PX = 6;

/** Lane height — must stay in sync with TimelinePane's LANE_HEIGHT. */
const LANE_HEIGHT = 38;
/** Ruler height — must stay in sync with TimelinePane's RULER_HEIGHT. */
const RULER_HEIGHT = 22;

export type EdgeHit = {
  /** Index of the track in `snapshot.tracks`. */
  trackIndex: number;
  /** Clip's index inside its track. */
  clipIndex: number;
  /** Clip's stable anchor uuid. */
  clipUuid: string;
  /** Which edge the pointer is near. */
  side: "start" | "end";
  /** The clip's current source-time bounds (from OTIO). Drag math
   *  computes new start/end deltas in source-time off these. */
  sourceStart: number;
  sourceEnd: number;
};

/**
 * Find the clip edge under `(canvasX, canvasY)`, if any. Returns
 * null when the pointer is outside any clip's edge zone (or in the
 * ruler / between lanes).
 *
 * Priority: when two adjacent clips both have an edge in the hit
 * zone, the one whose edge x is closest to the pointer wins. That
 * makes scrubbing along a tight cut feel right (you grab the edge
 * you visually pointed at, not the one whose lane number happens
 * to come first).
 */
export function hitTestEdge(
  canvasX: number,
  canvasY: number,
  snapshot: TimelineSnapshot,
  pps: number,
): EdgeHit | null {
  if (canvasY < RULER_HEIGHT) return null;
  const trackIndex = Math.floor((canvasY - RULER_HEIGHT) / LANE_HEIGHT);
  if (trackIndex < 0 || trackIndex >= snapshot.tracks.length) return null;
  const track = snapshot.tracks[trackIndex];

  let best: EdgeHit | null = null;
  let bestDx = EDGE_HIT_PX + 1;

  for (const item of track.items) {
    if (item.kind !== "clip") continue;
    const startX = item.track_start_s * pps;
    const endX = Math.max(
      startX + 2,
      (item.track_start_s + item.duration_s) * pps,
    );
    if (endX - startX <= EDGE_HIT_PX * 2) continue;
    const dStart = Math.abs(canvasX - startX);
    const dEnd = Math.abs(canvasX - endX);
    const sourceStart = item.source_start_s ?? 0;
    const sourceEnd = sourceStart + item.duration_s;

    if (dStart <= EDGE_HIT_PX && dStart < bestDx) {
      best = {
        trackIndex,
        clipIndex: item.index,
        clipUuid: item.clip_uuid,
        side: "start",
        sourceStart,
        sourceEnd,
      };
      bestDx = dStart;
    }
    if (dEnd <= EDGE_HIT_PX && dEnd < bestDx) {
      best = {
        trackIndex,
        clipIndex: item.index,
        clipUuid: item.clip_uuid,
        side: "end",
        sourceStart,
        sourceEnd,
      };
      bestDx = dEnd;
    }
  }
  return best;
}

/**
 * Convert a horizontal mouse delta (pixels) to a source-time delta
 * in seconds. The track-time and source-time axes are 1:1 within a
 * single clip — moving the right edge left by N pixels narrows
 * source_end by `N / pps` seconds.
 */
export function pxDeltaToSourceDelta(deltaPx: number, pps: number): number {
  return deltaPx / Math.max(0.001, pps);
}

/**
 * Find the clip item by track + clip index — the user-trim drag
 * looks up the clip on every move to recompute the proposed
 * start/end against the *current* snapshot (which can shift if the
 * agent edits in parallel).
 */
export function findClipItem(
  snapshot: TimelineSnapshot,
  trackIndex: number,
  clipIndex: number,
): TimelineItem | null {
  const track = snapshot.tracks[trackIndex];
  if (!track) return null;
  return track.items.find((it) => it.index === clipIndex) ?? null;
}

/**
 * Find the clip body under `(canvasX, canvasY)`, if any. Used by
 * the click-to-select branch in onPointerDown to populate the
 * properties pane. Returns the matching `{ trackIndex, clipIndex }`
 * when the pointer is inside a clip rect — anywhere inside, not
 * just the edges. Returns null when the pointer is in the ruler,
 * between lanes, or over a gap / transition / empty area.
 */
export function hitTestClipBody(
  canvasX: number,
  canvasY: number,
  snapshot: TimelineSnapshot,
  pps: number,
): { trackIndex: number; clipIndex: number } | null {
  if (canvasY < RULER_HEIGHT) return null;
  const trackIndex = Math.floor((canvasY - RULER_HEIGHT) / LANE_HEIGHT);
  if (trackIndex < 0 || trackIndex >= snapshot.tracks.length) return null;
  const track = snapshot.tracks[trackIndex];
  for (const item of track.items) {
    if (item.kind !== "clip") continue;
    const startX = item.track_start_s * pps;
    const endX = Math.max(
      startX + 2,
      (item.track_start_s + item.duration_s) * pps,
    );
    if (canvasX >= startX && canvasX <= endX) {
      return { trackIndex, clipIndex: item.index };
    }
  }
  return null;
}
