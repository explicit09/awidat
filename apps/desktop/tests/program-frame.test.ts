import assert from "node:assert/strict";
import { aspectRatioLabel, containedProgramFrame } from "../src/media/programFrame.ts";

assert.deepEqual(containedProgramFrame(1000, 1000, 1920, 1080), {
  left: 0,
  top: 218.75,
  width: 1000,
  height: 562.5,
});

assert.deepEqual(containedProgramFrame(1000, 400, 1920, 1080), {
  left: 144.44444444444446,
  top: 0,
  width: 711.1111111111111,
  height: 400,
});

assert.deepEqual(containedProgramFrame(360, 720, 1080, 1920), {
  left: 0,
  top: 40,
  width: 360,
  height: 640,
});

// Aspect match: the picture fills the monitor edge-to-edge — no
// matte, no inflation. Regression guard for the preview crop hacks.
assert.deepEqual(containedProgramFrame(1600, 900, 1920, 1080), {
  left: 0,
  top: 0,
  width: 1600,
  height: 900,
});

assert.equal(containedProgramFrame(0, 720, 1080, 1920), null);
assert.equal(containedProgramFrame(360, 720, 0, 1920), null);

assert.equal(aspectRatioLabel(1920, 1080), "16:9");
assert.equal(aspectRatioLabel(1080, 1920), "9:16");
assert.equal(aspectRatioLabel(640, 480), "4:3");
assert.equal(aspectRatioLabel(1080, 1080), "1:1");
// Continuity-camera podcast footage — irreducible, falls to decimal.
assert.equal(aspectRatioLabel(2122, 1440), "1.47:1");
assert.equal(aspectRatioLabel(0, 1080), null);
assert.equal(aspectRatioLabel(1920, Number.NaN), null);

console.log("program-frame geometry ok");
