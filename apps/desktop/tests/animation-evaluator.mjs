import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
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
