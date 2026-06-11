// Map a clip's `montage.color_correction` values onto a CSS `filter`
// string for the preview <video> element.
//
// This is an approximation by design: CSS filters can express
// exposure (brightness), contrast, and saturation faithfully enough
// for the Inspector's sliders to feel live, but have no faithful
// primitive for temperature, tint, shadows, or highlights — those
// apply in renders only (and in GPU transition preview frames).
// Approximating temperature with hue-rotate/sepia was rejected: it
// shifts skin tones in ways that misrepresent the final grade, which
// is worse than showing nothing.
//
// Pure string math, no DOM — unit-tested in
// tests/color-preview-filter.test.ts.

import type { ColorCorrectionStyling } from "../protocol";

/** Slider fields the CSS preview can and cannot represent — the
 *  Inspector uses this to hint which tweaks are render-only. */
export const CSS_PREVIEWABLE_FIELDS = [
  "exposure_ev",
  "contrast",
  "saturation",
] as const;
export const RENDER_ONLY_FIELDS = [
  "temperature",
  "tint",
  "shadows",
  "highlights",
] as const;

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}

function num(value: number | null | undefined, fallback: number): number {
  return Number.isFinite(value) ? (value as number) : fallback;
}

/** Round for stable, short CSS values (and stable equality checks —
 *  the caller skips reassigning `style.filter` when unchanged). */
function round(value: number): number {
  return Math.round(value * 10000) / 10000;
}

/** Build the CSS filter for a clip's color correction. Returns ""
 *  when there is no correction or every previewable field is at its
 *  resting value — callers assign the result to `style.filter`
 *  directly, so "" clears a stale filter. */
export function colorPreviewCssFilter(
  cc: ColorCorrectionStyling | null | undefined,
): string {
  if (!cc) return "";
  const parts: string[] = [];
  const exposureEv = clamp(num(cc.exposure_ev, 0), -4, 4);
  if (Math.abs(exposureEv) > 0.001) {
    // Exposure is photographic stops: ±1 EV doubles/halves light.
    parts.push(`brightness(${round(2 ** exposureEv)})`);
  }
  const contrast = clamp(num(cc.contrast, 1), 0, 3);
  if (Math.abs(contrast - 1) > 0.001) {
    parts.push(`contrast(${round(contrast)})`);
  }
  const saturation = clamp(num(cc.saturation, 1), 0, 3);
  if (Math.abs(saturation - 1) > 0.001) {
    parts.push(`saturate(${round(saturation)})`);
  }
  return parts.join(" ");
}
