// Generates crates/render/tests/fixtures/animation-vectors.json from the TypeScript
// animation evaluator (apps/desktop/src/timeline/animation.ts). Both the TS
// test (test:animation-vectors) and the Rust test
// (crates/eval/tests/animation_vectors.rs) replay these vectors so that
// preview (TS) and export (Rust) evaluators are pinned to the same numbers.
//
// Run after `npm run test:animation` has produced dist-test/ (this script
// imports the compiled evaluator directly, same as animation-evaluator.mjs):
//
//   node tests/generate-animation-vectors.mjs
//
// This OVERWRITES crates/render/tests/fixtures/animation-vectors.json. Only re-run it
// deliberately -- if TS and Rust disagree, regenerating hides the divergence
// instead of surfacing it.

import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { evaluateAnimationValueForTest } from "../dist-test/timeline/animation.js";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const OUT_PATH = path.resolve(
  __dirname,
  "../../../crates/render/tests/fixtures/animation-vectors.json",
);
const PRECISION = 1e-6;

function round(value) {
  return Math.round(value / PRECISION) * PRECISION;
}

function animation(parameter, keyframes, extrapolation = {}) {
  return {
    id: `vector-${parameter}`,
    target: { clip_id: "clip-a", parameter },
    keyframes,
    pre_extrapolation: extrapolation.pre ?? null,
    post_extrapolation: extrapolation.post ?? null,
    rationale: null,
  };
}

function kf(time_s, value, interpolation, extra = {}) {
  return { time_s, value, interpolation, easing: "linear", ...extra };
}

const cases = [];

function addCase(name, parameter, keyframes, times, extrapolation = {}) {
  const anim = animation(parameter, keyframes, extrapolation);
  const samples = times.map((t) => ({
    t,
    expected: round(evaluateAnimationValueForTest(anim, t)),
  }));
  cases.push({
    name,
    param: parameter,
    keyframes,
    pre_extrapolation: anim.pre_extrapolation,
    post_extrapolation: anim.post_extrapolation,
    samples,
  });
}

// --- 1. Linear interpolation across a spread of runtime params ---
addCase(
  "title.opacity linear fade in",
  "title.opacity",
  [kf(0, 0, "linear"), kf(1, 1, "linear")],
  [-0.5, 0, 0.25, 0.5, 0.75, 1, 1.5],
);

addCase(
  "title.x linear pan",
  "title.x",
  [kf(0, -0.2, "linear"), kf(2, 0.2, "linear")],
  [0, 1, 2],
);

addCase(
  "title.y linear pan",
  "title.y",
  [kf(0, 0.1, "linear"), kf(1.5, -0.1, "linear")],
  [0, 0.75, 1.5],
);

addCase(
  "title.font_size linear grow",
  "title.font_size",
  [kf(0, 32, "linear"), kf(1, 64, "linear")],
  [0, 0.5, 1],
);

addCase(
  "overlay.opacity linear fade",
  "overlay.opacity",
  [kf(0, 1, "linear"), kf(1, 0, "linear")],
  [0, 0.3, 1],
);

addCase(
  "overlay.x linear slide",
  "overlay.x",
  [kf(0, 0, "linear"), kf(1, 1, "linear")],
  [0, 0.5, 1],
);

addCase(
  "overlay.y linear slide",
  "overlay.y",
  [kf(0, 0, "linear"), kf(1, -1, "linear")],
  [0, 0.5, 1],
);

addCase(
  "overlay.scale linear zoom",
  "overlay.scale",
  [kf(0, 1, "linear"), kf(2, 1.5, "linear")],
  [0, 1, 2],
);

addCase(
  "overlay.rotation_deg linear spin",
  "overlay.rotation_deg",
  [kf(0, 0, "linear"), kf(1, 45, "linear")],
  [0, 0.5, 1],
);

addCase(
  "overlay.blur linear settle",
  "overlay.blur",
  [kf(0, 8, "linear"), kf(1, 0, "linear")],
  [0, 0.25, 1],
);

addCase(
  "montage.blur.radius_px linear",
  "montage.blur.radius_px",
  [kf(0, 12, "linear"), kf(1, 4, "linear")],
  [0, 0.5, 1],
);

addCase(
  "montage.shake.intensity_px linear",
  "montage.shake.intensity_px",
  [kf(0, 0, "linear"), kf(0.5, 6, "linear")],
  [0, 0.25, 0.5],
);

addCase(
  "montage.shake.frequency_hz linear",
  "montage.shake.frequency_hz",
  [kf(0, 2, "linear"), kf(0.5, 10, "linear")],
  [0, 0.25, 0.5],
);

addCase(
  "montage.warp.k1 linear",
  "montage.warp.k1",
  [kf(0, -0.1, "linear"), kf(1, 0.1, "linear")],
  [0, 0.5, 1],
);

addCase(
  "montage.warp.k2 linear",
  "montage.warp.k2",
  [kf(0, 0, "linear"), kf(1, 0.05, "linear")],
  [0, 0.5, 1],
);

addCase(
  "montage.warp.center_x linear",
  "montage.warp.center_x",
  [kf(0, 0.4, "linear"), kf(1, 0.6, "linear")],
  [0, 0.5, 1],
);

addCase(
  "montage.warp.center_y linear",
  "montage.warp.center_y",
  [kf(0, 0.4, "linear"), kf(1, 0.6, "linear")],
  [0, 0.5, 1],
);

// --- 2. Non-linear interpolation kinds ---
addCase(
  "overlay.opacity hold interpolation",
  "overlay.opacity",
  [kf(0, 0.2, "hold"), kf(1, 1, "linear")],
  [0, 0.5, 0.999],
);

addCase(
  "title.opacity step interpolation",
  "title.opacity",
  [kf(0, 0, "step"), kf(1, 1, "linear")],
  [0, 0.25, 0.49, 0.5, 0.75],
);

addCase(
  "overlay.scale bezier ease",
  "overlay.scale",
  [
    kf(0, 1, "bezier", {
      bezier: { out_x: 0.25, out_y: 0.1, in_x: 0.75, in_y: 0.9 },
    }),
    kf(1, 1.4, "linear"),
  ],
  [0, 0.25, 0.5, 0.75, 1],
);

addCase(
  "overlay.rotation_deg bezier with flat tangent",
  "overlay.rotation_deg",
  [
    kf(0, 0, "bezier", {
      bezier: { out_x: 0.25, out_y: 0.9, in_x: 0.75, in_y: 0.1 },
      tangent_mode: "flat",
    }),
    kf(1, 90, "linear", { tangent_mode: "flat" }),
  ],
  [0, 0.01, 0.5, 0.99, 1],
);

addCase(
  "overlay.x spring interpolation (underdamped)",
  "overlay.x",
  [
    kf(0, 0, "spring", {
      spring: { mass: 1, stiffness: 100, damping: 8 },
    }),
    kf(1, 1, "linear"),
  ],
  [0, 0.25, 0.5, 0.75, 1],
);

addCase(
  "overlay.y spring interpolation (near-critical)",
  "overlay.y",
  [
    kf(0, 0, "spring", {
      spring: { mass: 1, stiffness: 100, damping: 19.99999 },
    }),
    kf(1, 1, "linear"),
  ],
  [0, 0.5, 1],
);

addCase(
  "montage.blur.radius_px spring interpolation (overdamped)",
  "montage.blur.radius_px",
  [
    kf(0, 10, "spring", {
      spring: { mass: 1, stiffness: 60, damping: 40 },
    }),
    kf(1, 2, "linear"),
  ],
  [0, 0.5, 1],
);

// --- 3. Named easing curves (spot check a representative subset) ---
addCase(
  "title.opacity ease_out_back",
  "title.opacity",
  [kf(0, 0, "linear", { easing: "ease_out_back" }), kf(1, 1, "linear")],
  [0, 0.5, 1],
);

addCase(
  "title.opacity ease_in_bounce",
  "title.opacity",
  [kf(0, 0, "linear", { easing: "ease_in_bounce" }), kf(1, 1, "linear")],
  [0, 0.25, 0.5, 1],
);

addCase(
  "overlay.opacity ease_in_out_elastic",
  "overlay.opacity",
  [kf(0, 0, "linear", { easing: "ease_in_out_elastic" }), kf(1, 1, "linear")],
  [0, 0.3, 0.5, 0.7, 1],
);

addCase(
  "overlay.scale ease_in_out_cubic multi-segment",
  "overlay.scale",
  [
    kf(0, 1, "linear", { easing: "ease_in_out_cubic" }),
    kf(1, 1.5, "linear", { easing: "ease_in_out_cubic" }),
    kf(2, 1, "linear"),
  ],
  [0, 0.5, 1, 1.5, 2],
);

// --- 4. Extrapolation before first / after last keyframe ---
addCase(
  "overlay.x hold extrapolation (default)",
  "overlay.x",
  [kf(1, 10, "linear"), kf(3, 20, "linear")],
  [0, 1, 3, 4],
);

addCase(
  "overlay.x linear extrapolation both ends",
  "overlay.x",
  [kf(1, 10, "linear"), kf(3, 20, "linear")],
  [0, 1, 3, 4],
  { pre: "linear", post: "linear" },
);

addCase(
  "title.font_size linear pre-extrapolation only",
  "title.font_size",
  [kf(1, 32, "linear"), kf(2, 48, "linear")],
  [0, 0.5, 1],
  { pre: "linear" },
);

addCase(
  "overlay.blur linear post-extrapolation only",
  "overlay.blur",
  [kf(0, 8, "linear"), kf(1, 2, "linear")],
  [1, 1.5, 2],
  { post: "linear" },
);

const fixture = { precision: PRECISION, cases };
writeFileSync(OUT_PATH, `${JSON.stringify(fixture, null, 2)}\n`);
console.log(`Wrote ${cases.length} cases to ${OUT_PATH}`);
