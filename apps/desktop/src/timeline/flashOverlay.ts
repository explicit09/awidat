// Canvas helper for the Wave 4 W4.6 Review-focus flash pass.
//
// The focus controller (`useFlashRanges`) holds a tiny set of time
// ranges that auto-clear after ~600ms. The TimelineSurface paints them
// on top of the rendered tracks: a soft cyan glow for clip-kind flashes
// and a thin amber pulse for transition-kind (which are 1-frame thin
// glyphs the standard glow would swallow).
//
// Why a dedicated helper:
//   * Keeps the paint loop in TimelineSurface readable — one function
//     call instead of inlining the styling decisions.
//   * Pure draw, no React, no zustand — easy to test in isolation.
//
// Coordinate contract: the same `pps` + `laneHeight` the renderer uses,
// matching `RULER_HEIGHT` for the top offset. Pass the *current* render
// values; the helper doesn't read from the store.

import { RULER_HEIGHT } from "./layout.ts";
import type { FlashRange } from "../state/focusController";

const CLIP_FLASH_FILL = "rgba(56, 189, 248, 0.22)";
const CLIP_FLASH_STROKE = "rgba(56, 189, 248, 0.85)";
const TRANSITION_FLASH_STROKE = "rgba(252, 211, 77, 0.95)";

/**
 * Paint a transient highlight on every range in `flashes`. Ranges
 * with a null `trackIndex` are painted as a vertical band across
 * every lane (used for "mixed" proposals where we can't pin to a
 * single track). The helper is idempotent across repaints — the
 * caller manages range lifetime via the focus-controller store.
 */
export function drawFlashRanges(
  ctx: CanvasRenderingContext2D,
  flashes: ReadonlyArray<FlashRange>,
  pps: number,
  laneHeight: number,
): void {
  if (flashes.length === 0) return;
  ctx.save();
  for (const flash of flashes) {
    const x = Math.round(flash.startS * pps);
    const w = Math.max(6, Math.round((flash.endS - flash.startS) * pps));
    if (flash.trackIndex === null) {
      paintTrackAgnosticBand(ctx, x, w, flash.kind);
      continue;
    }
    const y = RULER_HEIGHT + flash.trackIndex * laneHeight + 4;
    const h = laneHeight - 8;
    if (flash.kind === "transition") {
      paintTransitionFlash(ctx, x, y, w, h);
    } else {
      paintClipFlash(ctx, x, y, w, h);
    }
  }
  ctx.restore();
}

function paintClipFlash(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
): void {
  ctx.fillStyle = CLIP_FLASH_FILL;
  ctx.fillRect(x, y, w, h);
  ctx.strokeStyle = CLIP_FLASH_STROKE;
  ctx.lineWidth = 2;
  ctx.strokeRect(x + 0.5, y + 0.5, w - 1, h - 1);
}

function paintTransitionFlash(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
): void {
  // Transition glyphs are thin; a fill would swallow them. We draw an
  // amber outline with a soft halo instead so the eye lands without
  // hiding the underlying glyph.
  ctx.shadowColor = "rgba(252, 211, 77, 0.65)";
  ctx.shadowBlur = 8;
  ctx.strokeStyle = TRANSITION_FLASH_STROKE;
  ctx.lineWidth = 1.5;
  ctx.strokeRect(x + 0.5, y + 0.5, w - 1, h - 1);
  ctx.shadowBlur = 0;
}

function paintTrackAgnosticBand(
  ctx: CanvasRenderingContext2D,
  x: number,
  w: number,
  kind: FlashRange["kind"],
): void {
  // No track? Paint a faint full-height band so a mixed-medium flash
  // still draws attention without picking a wrong lane.
  ctx.fillStyle =
    kind === "transition"
      ? "rgba(252, 211, 77, 0.16)"
      : "rgba(56, 189, 248, 0.16)";
  // Start under the ruler; height handled by the surrounding canvas
  // because we don't know the total track count here. The canvas is
  // already sized to the track stack so a tall rect clamps naturally.
  ctx.fillRect(x, RULER_HEIGHT, w, 2048);
}
