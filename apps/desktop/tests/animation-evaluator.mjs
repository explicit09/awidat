import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  evaluateAnimationValueForTest,
  evaluateAnimations,
} from "../dist-test/timeline/animation.js";
import { videoOverlayStyle } from "../dist-test/media/videoOverlayStyle.js";

const fade = {
  id: "fade",
  target: { clip_id: "clip-a", parameter: "title.opacity" },
  keyframes: [
    { time_s: 0, value: 0, interpolation: "linear", easing: "linear" },
    { time_s: 1, value: 1, interpolation: "linear", easing: "linear" },
  ],
  rationale: null,
};

assert.equal(evaluateAnimationValueForTest(fade, -1), 0);
assert.equal(evaluateAnimationValueForTest(fade, 0.5), 0.5);
assert.equal(evaluateAnimationValueForTest(fade, 2), 1);

const hold = {
  ...fade,
  keyframes: [
    { time_s: 0, value: 2, interpolation: "hold", easing: "linear" },
    { time_s: 1, value: 4, interpolation: "linear", easing: "linear" },
  ],
};

assert.equal(evaluateAnimationValueForTest(hold, 0.5), 2);

const step = {
  ...fade,
  keyframes: [
    { time_s: 0, value: 2, interpolation: "step", easing: "linear" },
    { time_s: 1, value: 4, interpolation: "linear", easing: "linear" },
  ],
};

assert.equal(evaluateAnimationValueForTest(step, 0.49), 2);
assert.equal(evaluateAnimationValueForTest(step, 0.5), 4);

const outBack = {
  ...fade,
  keyframes: [
    { time_s: 0, value: 0, interpolation: "linear", easing: "ease_out_back" },
    { time_s: 1, value: 1, interpolation: "linear", easing: "linear" },
  ],
};

const inBounce = {
  ...fade,
  keyframes: [
    { time_s: 0, value: 0, interpolation: "linear", easing: "ease_in_bounce" },
    { time_s: 1, value: 1, interpolation: "linear", easing: "linear" },
  ],
};

assert.ok(Math.abs(evaluateAnimationValueForTest(outBack, 0.5) - 1.0876975) < 1e-7);
assert.ok(Math.abs(evaluateAnimationValueForTest(inBounce, 0.25) - 0.02734375) < 1e-9);

const extrapolated = {
  ...fade,
  pre_extrapolation: "linear",
  post_extrapolation: "linear",
  keyframes: [
    { time_s: 1, value: 10, interpolation: "linear", easing: "linear" },
    { time_s: 3, value: 20, interpolation: "linear", easing: "linear" },
  ],
};

assert.equal(evaluateAnimationValueForTest(extrapolated, 0), 5);
assert.equal(evaluateAnimationValueForTest(extrapolated, 4), 25);

const spring = {
  ...fade,
  keyframes: [
    {
      time_s: 0,
      value: 0,
      interpolation: "spring",
      easing: "linear",
      spring: { mass: 1, stiffness: 100, damping: 12 },
    },
    { time_s: 1, value: 1, interpolation: "linear", easing: "linear" },
  ],
};
const springValue = evaluateAnimationValueForTest(spring, 0.5);
assert.ok(springValue > 0);
assert.notEqual(springValue, 0.5);

const nearCriticalSpring = {
  ...spring,
  keyframes: [
    {
      time_s: 0,
      value: 0,
      interpolation: "spring",
      easing: "linear",
      spring: { mass: 1, stiffness: 100, damping: 19.99999 },
    },
    { time_s: 1, value: 1, interpolation: "linear", easing: "linear" },
  ],
};
const nearCriticalValue = evaluateAnimationValueForTest(nearCriticalSpring, 0.5);
assert.ok(Number.isFinite(nearCriticalValue));
assert.ok(nearCriticalValue >= 0 && nearCriticalValue <= 1);

const flatTangent = {
  ...fade,
  keyframes: [
    {
      time_s: 0,
      value: 0,
      interpolation: "bezier",
      easing: "linear",
      bezier: { out_x: 0.25, out_y: 0.9, in_x: 0.75, in_y: 0.1 },
      tangent_mode: "flat",
    },
    {
      time_s: 1,
      value: 1,
      interpolation: "linear",
      easing: "linear",
      tangent_mode: "flat",
    },
  ],
};
assert.ok(evaluateAnimationValueForTest(flatTangent, 0.01) < 0.01);
assert.ok(evaluateAnimationValueForTest(flatTangent, 0.99) > 0.99);

const pathValues = evaluateAnimations(
  [
    {
      id: "path",
      target: { clip_id: "clip-a", parameter: "overlay.position" },
      keyframes: [],
      motion_path: {
        points: [
          { time_s: 0, x: -0.2, y: 0 },
          { time_s: 1, x: 0.2, y: 0.4 },
        ],
      },
      rationale: null,
    },
  ],
  0.5,
);
assert.equal(pathValues["overlay.x"], 0);
assert.equal(pathValues["overlay.y"], 0.2);

const curvedPathValues = evaluateAnimations(
  [
    {
      id: "curved-path",
      target: { clip_id: "clip-a", parameter: "overlay.position" },
      keyframes: [],
      motion_path: {
        points: [
          { time_s: 0, x: 0, y: 0, outgoing_control: { x: 0, y: 1 } },
          { time_s: 1, x: 1, y: 1, incoming_control: { x: 1, y: 0 } },
        ],
      },
      rationale: null,
    },
  ],
  0.25,
);
assert.equal(curvedPathValues["overlay.x"], 0.15625);
assert.equal(curvedPathValues["overlay.y"], 0.4375);

const easedPathValues = evaluateAnimations(
  [
    {
      id: "eased-path",
      target: { clip_id: "clip-a", parameter: "overlay.position" },
      keyframes: [
        {
          time_s: 0,
          value: 0,
          interpolation: "bezier",
          easing: "linear",
          bezier: { out_x: 0.25, out_y: 0.9, in_x: 0.75, in_y: 0.1 },
        },
        { time_s: 1, value: 1, interpolation: "linear", easing: "linear" },
      ],
      motion_path: {
        points: [
          { time_s: 0, x: -0.2, y: 0 },
          { time_s: 1, x: 0.2, y: 0.4 },
        ],
      },
      rationale: null,
    },
  ],
  0.25,
);
assert.ok(easedPathValues["overlay.x"] > -0.1);
assert.ok(easedPathValues["overlay.y"] > 0.1);

const runtimeParityValues = evaluateAnimations(
  [
    {
      id: "font-size",
      target: { clip_id: "clip-a", parameter: "title.font_size" },
      keyframes: [
        { time_s: 0, value: 36, interpolation: "linear", easing: "linear" },
        { time_s: 1, value: 48, interpolation: "linear", easing: "linear" },
      ],
      rationale: null,
    },
    {
      id: "rotation",
      target: { clip_id: "clip-a", parameter: "overlay.rotation_deg" },
      keyframes: [
        { time_s: 0, value: 0, interpolation: "linear", easing: "linear" },
        { time_s: 1, value: 12, interpolation: "linear", easing: "linear" },
      ],
      rationale: null,
    },
    {
      id: "blur",
      target: { clip_id: "clip-a", parameter: "overlay.blur" },
      keyframes: [
        { time_s: 0, value: 0, interpolation: "linear", easing: "linear" },
        { time_s: 1, value: 8, interpolation: "linear", easing: "linear" },
      ],
      rationale: null,
    },
  ],
  0.5,
);
assert.equal(runtimeParityValues["title.font_size"], 42);
assert.equal(runtimeParityValues["overlay.rotation_deg"], 6);
assert.equal(runtimeParityValues["overlay.blur"], 4);

const pipStyle = videoOverlayStyle(
  {
    mode: "pip",
    corner: "top_right",
    scale: 0.25,
    marginPct: 0.04,
    zIndex: 1,
    timelineStart: 10,
    previewHeightPx: 540,
    animations: [
      {
        id: "pip-punch",
        target: { clip_id: "clip-a", parameter: "overlay.scale" },
        keyframes: [
          { time_s: 0, value: 1, interpolation: "linear", easing: "linear" },
          { time_s: 1, value: 1.2, interpolation: "linear", easing: "linear" },
        ],
        rationale: null,
      },
    ],
  },
  10.5,
);
assert.equal(pipStyle.width, "25%");
assert.match(pipStyle.transform, /scale\(1\.1\)/);

const blurredPipStyle = videoOverlayStyle(
  {
    mode: "pip",
    corner: "top_right",
    scale: 0.25,
    marginPct: 0.04,
    zIndex: 1,
    timelineStart: 10,
    previewHeightPx: 540,
    animations: [
      {
        id: "pip-blur",
        target: { clip_id: "clip-a", parameter: "overlay.blur" },
        keyframes: [
          { time_s: 0, value: 0, interpolation: "linear", easing: "linear" },
          { time_s: 1, value: 6, interpolation: "linear", easing: "linear" },
        ],
        rationale: null,
      },
    ],
  },
  10.5,
);
assert.equal(blurredPipStyle.filter, "blur(1.5px)");

const fixturePath = new URL("../../../fixtures/motion/animation-parity.json", import.meta.url);
const fixture = JSON.parse(readFileSync(fixturePath, "utf8"));

for (const testCase of fixture.cases) {
  const animation = {
    id: testCase.name,
    target: { clip_id: "clip-a", parameter: "overlay.opacity" },
    keyframes: testCase.keyframes,
    rationale: null,
  };
  for (const sample of testCase.samples) {
    const actual = evaluateAnimationValueForTest(animation, sample.time_s);
    assert.ok(
      Math.abs(actual - sample.expected) < 1e-9,
      `${testCase.name} at ${sample.time_s}s expected ${sample.expected}, got ${actual}`,
    );
  }
}
