/** Pixels-per-second at zoom=1. Tuned so a 60s project fits the default pane width. */
export const PX_PER_SECOND_BASE = 12;

/** Height of one track lane in pixels. Editor-grade density: 62 leaves
 *  room for the filmstrip + edit handles + badges without crowding.
 *  Multiplied by store.trackZoom at paint time (see TimelineSurface). */
export const LANE_HEIGHT = 62;

/** Height of the time ruler at the top of the canvas. */
export const RULER_HEIGHT = 22;

/** Fixed left rail for track labels. Timeline media starts after this rail. */
export const TRACK_HEADER_WIDTH = 52;

/** Padding inside each clip block. */
export const CLIP_PADDING_X = 6;

export function computePps(durationS: number, cssWidth: number, zoom: number): number {
  const fitPps =
    durationS > 0
      ? Math.max(0.05, (cssWidth - TRACK_HEADER_WIDTH - 8) / durationS)
      : PX_PER_SECOND_BASE;
  return Math.min(fitPps * zoom, PX_PER_SECOND_BASE * 8);
}

export function timeToX(timeS: number, pps: number): number {
  return TRACK_HEADER_WIDTH + timeS * pps;
}

export function xToTime(x: number, pps: number): number {
  return Math.max(0, (x - TRACK_HEADER_WIDTH) / Math.max(0.001, pps));
}
