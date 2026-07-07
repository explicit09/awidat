// Replays crates/eval/fixtures/animation-vectors.json through the TS
// animation evaluator. This is the TS half of Task 9's TS<->Rust parity
// pin: crates/eval/tests/animation_vectors.rs replays the same JSON through
// the Rust evaluator (crates/render/src/animation.rs). Any future edit to
// either evaluator that changes a shared case's numbers will fail one side
// or the other instead of shipping silent preview/export drift.
//
// Vectors are generated (not hand-written) by
// tests/generate-animation-vectors.mjs, which calls this same evaluator --
// so this test is expected to be trivially green today. Its job is to catch
// *future* regressions, and to give the Rust side a target to match.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { evaluateAnimationValueForTest } from "../dist-test/timeline/animation.js";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const FIXTURE_PATH = path.resolve(
  __dirname,
  "../../../crates/eval/fixtures/animation-vectors.json",
);
const TOLERANCE = 1e-6;

type Keyframe = {
  time_s: number;
  value: number;
  interpolation: string;
  easing: string;
  bezier?: { out_x: number; out_y: number; in_x: number; in_y: number } | null;
  tangent_mode?: string;
  spring?: { mass: number; stiffness: number; damping: number } | null;
};

type Case = {
  name: string;
  param: string;
  keyframes: Keyframe[];
  pre_extrapolation: string | null;
  post_extrapolation: string | null;
  samples: { t: number; expected: number }[];
};

type Fixture = {
  precision: number;
  cases: Case[];
};

const fixture: Fixture = JSON.parse(readFileSync(FIXTURE_PATH, "utf8"));

assert.ok(fixture.cases.length >= 20, "expected at least 20 vector cases");

let checked = 0;
for (const testCase of fixture.cases) {
  const animation = {
    id: `vector-${testCase.param}`,
    target: { clip_id: "clip-a", parameter: testCase.param },
    keyframes: testCase.keyframes,
    pre_extrapolation: testCase.pre_extrapolation,
    post_extrapolation: testCase.post_extrapolation,
    rationale: null,
  };

  for (const sample of testCase.samples) {
    const actual = evaluateAnimationValueForTest(animation as never, sample.t);
    assert.ok(actual !== null, `${testCase.name} at t=${sample.t} produced null`);
    const diff = Math.abs((actual as number) - sample.expected);
    assert.ok(
      diff < TOLERANCE,
      `${testCase.name} at t=${sample.t}: expected ${sample.expected}, got ${actual} (diff ${diff})`,
    );
    checked += 1;
  }
}

console.log(`animation-vectors: replayed ${checked} samples across ${fixture.cases.length} cases`);
