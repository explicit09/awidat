import { strict as assert } from "node:assert";
import {
  CURVE_LUT_SIZE,
  applyGradeToRgb,
  buildCurveLut,
  buildGradePlan,
  isDefaultGrade,
  lutToRgba8,
  naturalCubicSpline,
} from "../src/media/gradeMath.ts";

const rest = {
  exposure_ev: 0,
  contrast: 1,
  saturation: 1,
  temperature: 0,
  tint: 0,
  shadows: 0,
  highlights: 0,
};

function near(a: number, b: number, eps = 1e-6) {
  assert.ok(Math.abs(a - b) <= eps, `expected ${a} ≈ ${b}`);
}

// --- buildGradePlan: stage gating mirrors the render chain ----------------

// Resting grade → no stages at all.
{
  const plan = buildGradePlan(rest);
  assert.equal(plan.eq, null);
  assert.equal(plan.curves, null);
  assert.equal(plan.colorBalance, null);
  assert.equal(isDefaultGrade(rest), true);
  assert.equal(isDefaultGrade(null), true);
}

// Shadows alone does NOT engage eq (matches the render: the eq filter
// only appears when exposure/contrast/saturation move) — but does
// engage curves.
{
  const plan = buildGradePlan({ ...rest, shadows: 0.5 });
  assert.equal(plan.eq, null);
  assert.ok(plan.curves);
  near(plan.curves.shadowMid, 0.25 + 0.5 * 0.16);
  assert.equal(plan.colorBalance, null);
}

// Exposure engages eq with the render's ADDITIVE brightness formula
// (exposure*0.14 + shadows*0.06 + highlights*0.03).
{
  const plan = buildGradePlan({ ...rest, exposure_ev: 1, shadows: 0.5, highlights: -0.25 });
  assert.ok(plan.eq);
  near(plan.eq.brightness, 1 * 0.14 + 0.5 * 0.06 + -0.25 * 0.03);
  near(plan.eq.contrast, 1);
  near(plan.eq.saturation, 1);
}

// Curve midpoints clamp to the render's bounds.
{
  const plan = buildGradePlan({ ...rest, shadows: 5, highlights: -5 });
  assert.ok(plan.curves);
  near(plan.curves.shadowMid, 0.45);
  near(plan.curves.highlightMid, 0.55);
}

// Temperature/tint map to the render's colorbalance channel offsets.
{
  const plan = buildGradePlan({ ...rest, temperature: 1, tint: 0.5 });
  assert.ok(plan.colorBalance);
  const cb = plan.colorBalance;
  near(cb.shadows[0], 1 * 0.1 + 0.5 * 0.045);
  near(cb.shadows[1], -0.5 * 0.09);
  near(cb.shadows[2], -1 * 0.1 + 0.5 * 0.045);
  near(cb.highlights[0], 1 * 0.07 + 0.5 * 0.035);
  near(cb.highlights[1], -0.5 * 0.07);
  near(cb.highlights[2], -1 * 0.07 + 0.5 * 0.035);
}

// --- naturalCubicSpline ----------------------------------------------------

// Passes exactly through its keypoints.
{
  const s = naturalCubicSpline([
    [0, 0],
    [0.25, 0.35],
    [0.75, 0.7],
    [1, 1],
  ]);
  near(s(0), 0);
  near(s(0.25), 0.35);
  near(s(0.75), 0.7);
  near(s(1), 1);
}

// Identity keypoints stay (numerically) the identity.
{
  const s = naturalCubicSpline([
    [0, 0],
    [0.25, 0.25],
    [0.75, 0.75],
    [1, 1],
  ]);
  for (const x of [0, 0.1, 0.33, 0.5, 0.9, 1]) near(s(x), x, 1e-9);
}

// Output clamps to [0,1] even for overshooting splines.
{
  const s = naturalCubicSpline([
    [0, 0],
    [0.25, 0.45],
    [0.75, 0.92],
    [1, 1],
  ]);
  for (let i = 0; i <= 100; i++) {
    const y = s(i / 100);
    assert.ok(y >= 0 && y <= 1);
  }
}

// --- buildCurveLut -----------------------------------------------------------

// Inactive stage → identity ramp.
{
  const lut = buildCurveLut(null);
  assert.equal(lut.length, CURVE_LUT_SIZE);
  assert.equal(lut[0], 0);
  assert.equal(lut[128], 128);
  assert.equal(lut[255], 255);
}

// Lifted shadows brighten the low end and pin the endpoints.
{
  const lut = buildCurveLut({ shadowMid: 0.35, highlightMid: 0.75 });
  assert.equal(lut[0], 0);
  assert.equal(lut[255], 255);
  assert.ok(lut[64] > 64, "lifted shadow midpoint should brighten 0.25");
}

// --- applyGradeToRgb reference chain ----------------------------------------

// Default plan is a no-op.
{
  const out = applyGradeToRgb(buildGradePlan(rest), [0.3, 0.5, 0.7]);
  near(out[0], 0.3);
  near(out[1], 0.5);
  near(out[2], 0.7);
}

// Warm temperature raises red and lowers blue on a dark pixel
// (shadow band weight is full strength below l≈0.2).
{
  const plan = buildGradePlan({ ...rest, temperature: 1 });
  const [r, , b] = applyGradeToRgb(plan, [0.15, 0.15, 0.15]);
  assert.ok(r > 0.15, "warm shadows gain red");
  assert.ok(b < 0.15, "warm shadows lose blue");
}

// Midgray sits outside both colorbalance bands → temperature leaves
// it untouched (band weights are zero at l=0.5).
{
  const plan = buildGradePlan({ ...rest, temperature: 1 });
  const out = applyGradeToRgb(plan, [0.5, 0.5, 0.5]);
  near(out[0], 0.5);
  near(out[2], 0.5);
}

// Positive exposure brightens via the additive eq path.
{
  const plan = buildGradePlan({ ...rest, exposure_ev: 1 });
  const out = applyGradeToRgb(plan, [0.5, 0.5, 0.5]);
  near(out[0], 0.5 + 0.14, 1e-3);
}

// Saturation 0 produces gray (chroma scaled to zero).
{
  const plan = buildGradePlan({ ...rest, saturation: 0.0001 });
  const [r, g, b] = applyGradeToRgb(plan, [0.8, 0.3, 0.2]);
  near(r, g, 1e-3);
  near(g, b, 1e-3);
}

// --- lutToRgba8 --------------------------------------------------------------

// A 2×2×2 identity-corner cube quantizes to RGBA8 with opaque alpha,
// preserving the Iridas R-fastest layout (texel i ← triplet i).
{
  // Triplets for corners (r,g,b) in R-fastest order.
  const table = [
    0, 0, 0,  1, 0, 0,  0, 1, 0,  1, 1, 0,
    0, 0, 1,  1, 0, 1,  0, 1, 1,  1, 1, 1,
  ];
  const rgba = lutToRgba8(table, 2);
  assert.equal(rgba.length, 8 * 4);
  // texel 1 = (r=1,g=0,b=0) corner → red.
  assert.deepEqual([...rgba.slice(4, 8)], [255, 0, 0, 255]);
  // texel 6 = (r=0,g=1,b=1) corner → cyan.
  assert.deepEqual([...rgba.slice(24, 28)], [0, 255, 255, 255]);
  // alpha always opaque.
  for (let i = 0; i < 8; i++) assert.equal(rgba[i * 4 + 3], 255);
}

// Out-of-range table values clamp instead of wrapping.
{
  const rgba = lutToRgba8([-0.5, 2, 0.5], 1);
  assert.deepEqual([...rgba], [0, 255, 128, 255]);
}

console.log("grade-math: all assertions passed");
