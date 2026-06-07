// Verifies the DaVinci-style timeline restyle by capturing the canvas
// fill/stroke styles drawTracks emits, without a real canvas. Records
// every fillStyle/strokeStyle assignment alongside the op that follows,
// then asserts the per-track clip colours, the clip-colour accent
// stripe, and the red-orange selection ring are painted.

import assert from "node:assert/strict";

import { drawTracks } from "./renderer.ts";
import type { TimelineItem } from "../protocol/generated/TimelineItem.ts";

/** A clip item with only the fields drawItem reads; cast through unknown
 *  so we don't have to spell out all ~30 generated fields. */
function clip(
  index: number,
  name: string,
  trackStartS: number,
  durationS: number,
  extra: Record<string, unknown> = {},
): TimelineItem {
  return {
    kind: "clip",
    index,
    name,
    clip_uuid: name,
    track_start_s: trackStartS,
    duration_s: durationS,
    source_start_s: 0,
    asset_id: `${name}.mp4`,
    waveform_path: null,
    thumbnail_dir: null,
    title: null,
    link_group_id: null,
    speed: null,
    ...extra,
  } as unknown as TimelineItem;
}

/** A canvas 2D context stub that records styled fills/strokes. */
function recordingCtx() {
  const fills: string[] = [];
  const strokes: string[] = [];
  let fillStyle = "";
  let strokeStyle = "";
  const ctx = {
    get fillStyle() {
      return fillStyle;
    },
    set fillStyle(v: string) {
      fillStyle = v;
    },
    get strokeStyle() {
      return strokeStyle;
    },
    set strokeStyle(v: string) {
      strokeStyle = v;
    },
    lineWidth: 1,
    font: "",
    textBaseline: "top",
    fillRect: () => fills.push(fillStyle),
    fillText: () => {},
    strokeRect: () => strokes.push(strokeStyle),
    beginPath: () => {},
    closePath: () => {},
    moveTo: () => {},
    lineTo: () => {},
    quadraticCurveTo: () => {},
    arc: () => {},
    fill: () => fills.push(fillStyle),
    stroke: () => strokes.push(strokeStyle),
    save: () => {},
    restore: () => {},
    clip: () => {},
    setLineDash: () => {},
    measureText: (t: string) => ({ width: t.length * 6 }) as TextMetrics,
    createLinearGradient: () => ({ addColorStop: () => {} }),
  } as unknown as CanvasRenderingContext2D;
  return { ctx, fills, strokes };
}

const tracks = [
  { name: "V1", kind: "video", role: null, items: [clip(0, "shotA", 0, 5)] },
  { name: "A1", kind: "audio", role: null, items: [clip(0, "voice", 0, 5)] },
];

console.log("# davinci clip colours");
{
  const { ctx, fills } = recordingCtx();
  drawTracks(ctx, 600, tracks, 20, {});
  // Video clip body — translucent blue-grey.
  assert.ok(
    fills.some((c) => c === "rgba(56, 78, 104, 0.55)"),
    "video clip should paint the blue-grey body",
  );
  // Audio clip body — translucent teal.
  assert.ok(
    fills.some((c) => c === "rgba(28, 74, 70, 0.55)"),
    "audio clip should paint the teal body",
  );
  // Per-track accent stripes.
  assert.ok(
    fills.some((c) => c === "rgba(125, 170, 222, 0.90)"),
    "video clip should paint the blue accent stripe",
  );
  assert.ok(
    fills.some((c) => c === "rgba(45, 196, 170, 0.90)"),
    "audio clip should paint the teal accent stripe",
  );
}
console.log("  ok  per-track clip body + accent stripe colours");

console.log("# davinci selection ring");
{
  const { ctx, strokes } = recordingCtx();
  drawTracks(ctx, 600, tracks, 20, { selectedKey: "0:0" });
  assert.ok(
    strokes.some((c) => c === "rgba(255, 96, 64, 0.95)"),
    "selected clip should paint the red-orange ring",
  );
  assert.ok(
    !strokes.includes("#EF4444"),
    "selection ring must no longer use the old cyan brand colour",
  );
}
console.log("  ok  red-orange selection ring replaces cyan");
