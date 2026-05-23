import { LANE_HEIGHT, RULER_HEIGHT, computePps } from "./layout.ts";
import type { TimelineSnapshot } from "./store.ts";

export type CanvasLayoutInput = {
  snapshot: TimelineSnapshot;
  proposalSnapshot: TimelineSnapshot | null;
  viewportWidth: number;
  zoom: number;
  /** Per-lane height in CSS px. Defaults to the static `LANE_HEIGHT`
   *  constant when the caller hasn't opted into runtime track zoom. */
  laneHeight?: number;
};

export type CanvasLayout = {
  cssHeight: number;
  cssWidth: number;
  lanesCount: number;
  pps: number;
  totalDuration: number;
  /** Resolved per-lane height in CSS px; passed downstream to renderer
   *  + hit-detect + handles so every reader uses the same value. */
  laneHeight: number;
};

export function computeCanvasLayout({
  snapshot,
  proposalSnapshot,
  viewportWidth,
  zoom,
  laneHeight = LANE_HEIGHT,
}: CanvasLayoutInput): CanvasLayout {
  const proposedTrackCount = proposalSnapshot?.tracks.length ?? 0;
  const lanesCount = Math.max(snapshot.tracks.length, proposedTrackCount, 1);
  const cssHeight = RULER_HEIGHT + lanesCount * laneHeight;
  const proposedDuration = proposalSnapshot?.duration_s ?? 0;
  const totalDuration = Math.max(snapshot.duration_s, proposedDuration);
  const pps = computePps(totalDuration, viewportWidth, zoom);
  const cssWidth =
    totalDuration > 0
      ? Math.max(viewportWidth, totalDuration * pps + 8)
      : viewportWidth;

  return {
    cssHeight,
    cssWidth,
    lanesCount,
    pps,
    totalDuration,
    laneHeight,
  };
}
