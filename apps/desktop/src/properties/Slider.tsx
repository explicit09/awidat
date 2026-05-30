// Studio-Pro Inspector slider primitive.
//
// A horizontal range input rendered with on-brand styling:
//   - 4px track (charcoal `--color-surface-input`)
//   - Cyan fill (`--color-brand-secondary`) from the zero-point to the thumb.
//     The zero-point defaults to `min`, but can be set to `defaultValue`
//     (e.g. 0 EV, 1× speed, 1× volume) so bipolar controls fill outward
//     from the canonical resting position.
//   - 14px circular thumb. Filled cyan when the value differs from
//     `defaultValue`; quieter muted grey when the control is at rest so
//     the user can see at a glance which knobs have been touched.
//
// Renders a paired value badge to the right via the surrounding row
// layout — callers wrap us in `.properties-control-row` and add their
// own `<span class="properties-control-value">` for the readout.
//
// Implementation note: track-fill is a CSS gradient computed inline.
// `accent-color` alone can't paint a fill that flips around an
// arbitrary zero point, so we draw the band manually.

import { type ChangeEvent } from "react";

export interface SliderProps {
  value: number;
  min: number;
  max: number;
  step: number;
  /** The value at which the thumb is considered "at rest" (untouched).
   *  Defaults to `min` so unipolar sliders fill rightward from the left.
   *  Bipolar sliders (e.g. Exposure ±4 EV) pass 0 here. */
  defaultValue?: number;
  onChange: (next: number) => void;
  ariaLabel?: string;
}

export function Slider({
  value,
  min,
  max,
  step,
  defaultValue,
  onChange,
  ariaLabel,
}: SliderProps) {
  const range = Math.max(0.0001, max - min);
  const zero = defaultValue ?? min;
  const valuePct = clamp01((value - min) / range) * 100;
  const zeroPct = clamp01((zero - min) / range) * 100;
  const fillStart = Math.min(valuePct, zeroPct);
  const fillEnd = Math.max(valuePct, zeroPct);
  const isAtRest = Math.abs(value - zero) < step / 2;

  // Stitch the fill into the track via a sharp-stop linear gradient.
  // The thumb gets its color via the data-rest attribute (handled in CSS).
  const trackBackground = `linear-gradient(to right,
      var(--color-surface-input) 0%,
      var(--color-surface-input) ${fillStart}%,
      var(--color-brand-secondary) ${fillStart}%,
      var(--color-brand-secondary) ${fillEnd}%,
      var(--color-surface-input) ${fillEnd}%,
      var(--color-surface-input) 100%)`;

  function handle(e: ChangeEvent<HTMLInputElement>) {
    const next = parseFloat(e.target.value);
    if (Number.isFinite(next)) onChange(next);
  }

  return (
    <input
      type="range"
      className="properties-slider-pro"
      data-rest={isAtRest ? "true" : "false"}
      min={min}
      max={max}
      step={step}
      value={value}
      onChange={handle}
      aria-label={ariaLabel}
      style={{ background: trackBackground }}
    />
  );
}

function clamp01(n: number): number {
  if (!Number.isFinite(n)) return 0;
  if (n < 0) return 0;
  if (n > 1) return 1;
  return n;
}
