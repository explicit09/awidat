/** Pixels-per-second at zoom=1. Tuned so a 60s project fits the default pane width. */
export const PX_PER_SECOND_BASE = 12;

/** Height of one track lane in pixels. */
export const LANE_HEIGHT = 38;

/** Height of the time ruler at the top of the canvas. */
export const RULER_HEIGHT = 22;

/** Padding inside each clip block. */
export const CLIP_PADDING_X = 6;

export function computePps(durationS: number, cssWidth: number, zoom: number): number {
  const fitPps =
    durationS > 0 ? Math.max(0.05, (cssWidth - 8) / durationS) : PX_PER_SECOND_BASE;
  return Math.min(fitPps * zoom, PX_PER_SECOND_BASE * 8);
}
