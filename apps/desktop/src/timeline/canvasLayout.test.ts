import assert from "node:assert/strict";
import { computeCanvasLayout } from "./canvasLayout.ts";
import type { TimelineSnapshot } from "./store.ts";

function snapshot(duration_s: number, trackCount: number): TimelineSnapshot {
  return {
    duration_s,
    broadcast_overlay: null,
    cut_boundaries: [],
    preview_limitations: [],
    tracks: Array.from({ length: trackCount }, (_, index) => ({
      name: `V${index + 1}`,
      kind: "video",
      role: null,
      audio: null,
      items: [],
    })),
  };
}

console.log("# computeCanvasLayout");
assert.deepEqual(
  computeCanvasLayout({
    snapshot: snapshot(10, 2),
    proposalSnapshot: null,
    viewportWidth: 200,
    zoom: 1,
  }),
  {
    cssHeight: 98,
    cssWidth: 200,
    lanesCount: 2,
    pps: 19.2,
    totalDuration: 10,
    laneHeight: 38,
  },
);
console.log("  ok  sizes the canvas from current timeline duration and track count");

assert.deepEqual(
  computeCanvasLayout({
    snapshot: snapshot(10, 1),
    proposalSnapshot: snapshot(20, 3),
    viewportWidth: 200,
    zoom: 1,
  }),
  {
    cssHeight: 136,
    cssWidth: 200,
    lanesCount: 3,
    pps: 9.6,
    totalDuration: 20,
    laneHeight: 38,
  },
);
console.log("  ok  uses proposed duration and track count when proposal is larger");

assert.deepEqual(
  computeCanvasLayout({
    snapshot: snapshot(0, 0),
    proposalSnapshot: null,
    viewportWidth: 320,
    zoom: 1,
  }),
  {
    cssHeight: 60,
    cssWidth: 320,
    lanesCount: 1,
    pps: 12,
    totalDuration: 0,
    laneHeight: 38,
  },
);
console.log("  ok  keeps an empty timeline at one visible lane");

// Track-zoom path: laneHeight propagates through height + back out
console.log("# computeCanvasLayout with explicit laneHeight");
assert.deepEqual(
  computeCanvasLayout({
    snapshot: snapshot(0, 0),
    proposalSnapshot: null,
    viewportWidth: 320,
    zoom: 1,
    laneHeight: 62,
  }),
  {
    cssHeight: 22 + 62, // RULER_HEIGHT + 1 lane @ 62
    cssWidth: 320,
    lanesCount: 1,
    pps: 12,
    totalDuration: 0,
    laneHeight: 62,
  },
);
console.log("  ok  honors caller-provided laneHeight and exports it");
