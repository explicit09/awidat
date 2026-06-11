// CPU-side math for the preview's WebGL grade pass.
//
// The render engine applies `montage.color_correction` as an FFmpeg
// filter chain (crates/render/src/timeline.rs, color_filter_chain):
//
//   1. eq=brightness=B:contrast=C:saturation=S
//      where B = clamp(exposure_ev*0.14 + shadows*0.06
//                      + highlights*0.03, -1, 1)
//      — applied in luma/chroma space: brightness is ADDITIVE on
//      luma (not photographic 2^ev), contrast pivots luma at 0.5,
//      saturation scales chroma.
//   2. curves=all='0/0 0.25/SM 0.75/HM 1/1'
//      where SM = clamp(0.25 + shadows*0.16,    0.08, 0.45)
//            HM = clamp(0.75 + highlights*0.16, 0.55, 0.92)
//      — natural cubic spline per RGB channel.
//   3. colorbalance=rs:gs:bs:rh:gh:bh
//      rs = clamp(temp*0.1  + tint*0.045, -1, 1)   gs = clamp(-tint*0.09,  -1, 1)
//      bs = clamp(-temp*0.1 + tint*0.045, -1, 1)   rh = clamp(temp*0.07 + tint*0.035, -1, 1)
//      gh = clamp(-tint*0.07, -1, 1)               bh = clamp(-temp*0.07 + tint*0.035, -1, 1)
//      — FFmpeg colorbalance band weights (vf_colorbalance.c):
//      lightness l = (max+min)/2, shadow weight
//      clamp((1/3 - l)*4 + 0.5, 0, 1)*0.7, highlight weight
//      clamp((l + 1/3 - 1)*4 + 0.5, 0, 1)*0.7.
//
// The preview must show what the export will look like, so this
// module mirrors those formulas exactly — each stage engages under
// the same conditions as the render chain. Stage params are computed
// here on the CPU (and unit-tested); the fragment shader consumes
// them as uniforms plus a sampled curve LUT.

import type { ColorCorrectionStyling } from "../protocol";

const EPS = 1e-9;

function n(value: number | null | undefined, fallback: number): number {
  return Number.isFinite(value) ? (value as number) : fallback;
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, v));
}

export type EqParams = {
  brightness: number;
  contrast: number;
  saturation: number;
};

export type CurveParams = { shadowMid: number; highlightMid: number };

export type ColorBalanceParams = {
  /** Per-channel shadow-band offsets (r, g, b). */
  shadows: [number, number, number];
  /** Per-channel highlight-band offsets (r, g, b). */
  highlights: [number, number, number];
};

export type GradePlan = {
  eq: EqParams | null;
  curves: CurveParams | null;
  colorBalance: ColorBalanceParams | null;
};

/** Mirror of the render chain's stage gating. `null` plan members
 *  mean the render would not emit that filter — the shader skips the
 *  stage rather than applying an identity that drifts. */
export function buildGradePlan(
  cc: ColorCorrectionStyling | null | undefined,
): GradePlan {
  if (!cc) return { eq: null, curves: null, colorBalance: null };
  const exposure = n(cc.exposure_ev, 0);
  const contrast = n(cc.contrast, 1);
  const saturation = n(cc.saturation, 1);
  const temperature = n(cc.temperature, 0);
  const tint = n(cc.tint, 0);
  const shadows = n(cc.shadows, 0);
  const highlights = n(cc.highlights, 0);

  let eq: EqParams | null = null;
  if (
    Math.abs(exposure) > EPS ||
    Math.abs(contrast - 1) > EPS ||
    Math.abs(saturation - 1) > EPS
  ) {
    eq = {
      brightness: clamp(
        exposure * 0.14 + shadows * 0.06 + highlights * 0.03,
        -1,
        1,
      ),
      contrast,
      saturation,
    };
  }

  let curves: CurveParams | null = null;
  if (Math.abs(shadows) > EPS || Math.abs(highlights) > EPS) {
    curves = {
      shadowMid: clamp(0.25 + shadows * 0.16, 0.08, 0.45),
      highlightMid: clamp(0.75 + highlights * 0.16, 0.55, 0.92),
    };
  }

  let colorBalance: ColorBalanceParams | null = null;
  if (Math.abs(temperature) > EPS || Math.abs(tint) > EPS) {
    colorBalance = {
      shadows: [
        clamp(temperature * 0.1 + tint * 0.045, -1, 1),
        clamp(-tint * 0.09, -1, 1),
        clamp(-temperature * 0.1 + tint * 0.045, -1, 1),
      ],
      highlights: [
        clamp(temperature * 0.07 + tint * 0.035, -1, 1),
        clamp(-tint * 0.07, -1, 1),
        clamp(-temperature * 0.07 + tint * 0.035, -1, 1),
      ],
    };
  }

  return { eq, curves, colorBalance };
}

export function isDefaultGrade(
  cc: ColorCorrectionStyling | null | undefined,
): boolean {
  const plan = buildGradePlan(cc);
  return plan.eq === null && plan.curves === null && plan.colorBalance === null;
}

/** Natural cubic spline through `points` (sorted by x, natural
 *  boundary: zero second derivative at the ends) — the interpolation
 *  FFmpeg's curves filter uses for its keypoints. Returns an
 *  evaluator clamped to [0, 1]. */
export function naturalCubicSpline(
  points: ReadonlyArray<readonly [number, number]>,
): (x: number) => number {
  const count = points.length;
  if (count < 2) {
    const y = count === 1 ? points[0][1] : 0;
    return () => clamp(y, 0, 1);
  }
  const xs = points.map((p) => p[0]);
  const ys = points.map((p) => p[1]);
  const segments = count - 1;
  const h = new Array<number>(segments);
  for (let i = 0; i < segments; i++) h[i] = xs[i + 1] - xs[i];

  // Solve the tridiagonal system for second derivatives (natural BC).
  const m = new Array<number>(count).fill(0);
  if (count > 2) {
    const inner = count - 2;
    const diag = new Array<number>(inner);
    const rhs = new Array<number>(inner);
    for (let i = 0; i < inner; i++) {
      diag[i] = 2 * (h[i] + h[i + 1]);
      rhs[i] =
        6 * ((ys[i + 2] - ys[i + 1]) / h[i + 1] - (ys[i + 1] - ys[i]) / h[i]);
    }
    // Thomas algorithm; sub/super diagonals are h[1..inner-1].
    for (let i = 1; i < inner; i++) {
      const w = h[i] / diag[i - 1];
      diag[i] -= w * h[i];
      rhs[i] -= w * rhs[i - 1];
    }
    m[inner] = rhs[inner - 1] / diag[inner - 1];
    for (let i = inner - 1; i >= 1; i--) {
      m[i] = (rhs[i - 1] - h[i] * m[i + 1]) / diag[i - 1];
    }
  }

  return (x: number): number => {
    const cx = clamp(x, xs[0], xs[count - 1]);
    let i = segments - 1;
    for (let k = 0; k < segments; k++) {
      if (cx <= xs[k + 1]) {
        i = k;
        break;
      }
    }
    const t = cx - xs[i];
    const hi = h[i];
    const a = (m[i + 1] - m[i]) / (6 * hi);
    const b = m[i] / 2;
    const c = (ys[i + 1] - ys[i]) / hi - (hi * (2 * m[i] + m[i + 1])) / 6;
    return clamp(ys[i] + t * (c + t * (b + t * a)), 0, 1);
  };
}

export const CURVE_LUT_SIZE = 256;

/** Sample the curves stage into a LUT the shader reads as a 256×1
 *  texture. Identity samples when the stage is inactive. */
export function buildCurveLut(curves: CurveParams | null): Uint8Array {
  const lut = new Uint8Array(CURVE_LUT_SIZE);
  if (!curves) {
    for (let i = 0; i < CURVE_LUT_SIZE; i++) lut[i] = i;
    return lut;
  }
  const spline = naturalCubicSpline([
    [0, 0],
    [0.25, curves.shadowMid],
    [0.75, curves.highlightMid],
    [1, 1],
  ]);
  for (let i = 0; i < CURVE_LUT_SIZE; i++) {
    lut[i] = Math.round(spline(i / (CURVE_LUT_SIZE - 1)) * 255);
  }
  return lut;
}

// ---------------------------------------------------------------------------
// CPU reference of the full chain. The fragment shader implements the
// same math on the GPU; this reference exists so the formulas are
// unit-testable in node and any future shader edit can be checked
// against it by hand.
// ---------------------------------------------------------------------------

const LUMA_R = 0.2126;
const LUMA_G = 0.7152;
const LUMA_B = 0.0722;

/** Apply the grade plan to one RGB pixel (components in [0,1]). */
export function applyGradeToRgb(
  plan: GradePlan,
  rgb: readonly [number, number, number],
): [number, number, number] {
  let [r, g, b] = rgb;

  if (plan.eq) {
    // FFmpeg eq works on luma/chroma: additive brightness + pivot-0.5
    // contrast on Y', chroma scaled by saturation (BT.709).
    const y = LUMA_R * r + LUMA_G * g + LUMA_B * b;
    const cb = (b - y) / 1.8556;
    const cr = (r - y) / 1.5748;
    const y2 = (y - 0.5) * plan.eq.contrast + 0.5 + plan.eq.brightness;
    const cb2 = cb * plan.eq.saturation;
    const cr2 = cr * plan.eq.saturation;
    r = y2 + 1.5748 * cr2;
    b = y2 + 1.8556 * cb2;
    g = (y2 - LUMA_R * r - LUMA_B * b) / LUMA_G;
    r = clamp(r, 0, 1);
    g = clamp(g, 0, 1);
    b = clamp(b, 0, 1);
  }

  if (plan.curves) {
    const spline = naturalCubicSpline([
      [0, 0],
      [0.25, plan.curves.shadowMid],
      [0.75, plan.curves.highlightMid],
      [1, 1],
    ]);
    r = spline(r);
    g = spline(g);
    b = spline(b);
  }

  if (plan.colorBalance) {
    const l = (Math.max(r, g, b) + Math.min(r, g, b)) / 2;
    const third = 1 / 3;
    const ws = clamp((third - l) * 4 + 0.5, 0, 1) * 0.7;
    const wh = clamp((l + third - 1) * 4 + 0.5, 0, 1) * 0.7;
    const cb = plan.colorBalance;
    r = clamp(r + cb.shadows[0] * ws + cb.highlights[0] * wh, 0, 1);
    g = clamp(g + cb.shadows[1] * ws + cb.highlights[1] * wh, 0, 1);
    b = clamp(b + cb.shadows[2] * ws + cb.highlights[2] * wh, 0, 1);
  }

  return [r, g, b];
}
