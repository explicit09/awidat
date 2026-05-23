// Pointer hit-detection helpers for the TimelinePane canvas.
//
// Given a pointer position in canvas-local coordinates, figure out
// whether the user is hovering near a clip edge. Drives both the
// cursor hint (`cursor: ew-resize` near edges) and the drag-to-trim
// pointer-down branch.

import type { TimelineSnapshot, TimelineItem } from "./store";

/** Pointer-pixel tolerance for "near a clip edge". */
export const EDGE_HIT_PX = 6;

/** Base lane height at trackZoom = 1. Hit-tests accept a `laneHeight`
 *  override so they stay in sync with the canvas when the user scales
 *  the vertical zoom. Kept in sync with layout.LANE_HEIGHT. */
export const LANE_HEIGHT_BASE = 62;
/** Ruler height — fixed, not affected by track zoom. */
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
  laneHeight: number = LANE_HEIGHT_BASE,
): EdgeHit | null {
  if (canvasY < RULER_HEIGHT) return null;
  const trackIndex = Math.floor((canvasY - RULER_HEIGHT) / laneHeight);
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

/** Result of hit-testing a *shared edit point* between two adjacent
 *  clips on the same track. The pointer is within `EDGE_HIT_PX` of
 *  both the outgoing clip's `end` and the incoming clip's `start`,
 *  AND those two times match within ~1 frame. Drives roll-edit
 *  gestures. */
export type BoundaryHit = {
  trackIndex: number;
  from: {
    clipIndex: number;
    clipUuid: string;
    sourceStart: number;
    sourceEnd: number;
  };
  to: {
    clipIndex: number;
    clipUuid: string;
    sourceStart: number;
    sourceEnd: number;
  };
};

/** Detect when the pointer is on the shared edit point between two
 *  adjacent clips on the same track. Returns null when the pointer
 *  is on an outer edge (no neighbor on that side) or not near any
 *  edge at all. */
export function hitTestBoundary(
  canvasX: number,
  canvasY: number,
  snapshot: TimelineSnapshot,
  pps: number,
  laneHeight: number = LANE_HEIGHT_BASE,
): BoundaryHit | null {
  if (canvasY < RULER_HEIGHT) return null;
  const trackIndex = Math.floor((canvasY - RULER_HEIGHT) / laneHeight);
  if (trackIndex < 0 || trackIndex >= snapshot.tracks.length) return null;
  const track = snapshot.tracks[trackIndex];
  const clips = track.items.filter((it) => it.kind === "clip");
  for (let i = 0; i < clips.length - 1; i += 1) {
    const a = clips[i];
    const b = clips[i + 1];
    if (a.kind !== "clip" || b.kind !== "clip") continue;
    const aEndS = a.track_start_s + a.duration_s;
    const bStartS = b.track_start_s;
    if (Math.abs(aEndS - bStartS) > 0.04) continue;
    const boundaryX = aEndS * pps;
    if (Math.abs(canvasX - boundaryX) <= EDGE_HIT_PX) {
      return {
        trackIndex,
        from: {
          clipIndex: a.index,
          clipUuid: a.clip_uuid,
          sourceStart: a.source_start_s ?? 0,
          sourceEnd: (a.source_start_s ?? 0) + a.duration_s,
        },
        to: {
          clipIndex: b.index,
          clipUuid: b.clip_uuid,
          sourceStart: b.source_start_s ?? 0,
          sourceEnd: (b.source_start_s ?? 0) + b.duration_s,
        },
      };
    }
  }
  return null;
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
  laneHeight: number = LANE_HEIGHT_BASE,
): { trackIndex: number; clipIndex: number } | null {
  if (canvasY < RULER_HEIGHT) return null;
  const trackIndex = Math.floor((canvasY - RULER_HEIGHT) / laneHeight);
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

/**
 * Find a selectable timeline item under `(canvasX, canvasY)`.
 * Clips and transitions are selectable; gaps remain passive timeline space.
 */
export function hitTestSelectableBody(
  canvasX: number,
  canvasY: number,
  snapshot: TimelineSnapshot,
  pps: number,
  laneHeight: number = LANE_HEIGHT_BASE,
): { trackIndex: number; clipIndex: number } | null {
  if (canvasY < RULER_HEIGHT) return null;
  const trackIndex = Math.floor((canvasY - RULER_HEIGHT) / laneHeight);
  if (trackIndex < 0 || trackIndex >= snapshot.tracks.length) return null;
  const track = snapshot.tracks[trackIndex];
  for (const item of track.items) {
    if (item.kind === "gap") continue;
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
