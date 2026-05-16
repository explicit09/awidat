import assert from "node:assert/strict";
import { evaluateAnimationValueForTest } from "../dist-test/timeline/animation.js";

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
