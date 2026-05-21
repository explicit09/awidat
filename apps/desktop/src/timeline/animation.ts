import type { TimelineParameterAnimation } from "../protocol";

export type AnimationValues = Record<string, number>;

type BezierHandles = {
  out_x: number;
  out_y: number;
  in_x: number;
  in_y: number;
};

type KeyframeWithBezier = TimelineParameterAnimation["keyframes"][number] & {
  bezier?: BezierHandles | null;
};

type SpringParameters = {
  mass: number;
  stiffness: number;
  damping: number;
};

type KeyframeWithSpring = TimelineParameterAnimation["keyframes"][number] & {
  spring?: SpringParameters | null;
};

type AnimationWithExtrapolation = TimelineParameterAnimation & {
  pre_extrapolation?: string | null;
  post_extrapolation?: string | null;
};

const PHASE_3A_PARAMETERS = new Set([
  "title.opacity",
  "title.x",
  "title.y",
  "title.position",
  "title.font_size",
  "overlay.opacity",
  "overlay.x",
  "overlay.y",
  "overlay.position",
  "overlay.scale",
  "overlay.rotation_deg",
]);

export function isPhase3AParameter(parameter: string): boolean {
  return PHASE_3A_PARAMETERS.has(parameter);
}

export function evaluateAnimationValue(
  animation: TimelineParameterAnimation,
  localTimeS: number,
): number | null {
  const keyframes = animation.keyframes;
  if (keyframes.length === 0) return null;
  const extrapolated = animation as AnimationWithExtrapolation;
  if (localTimeS <= keyframes[0].time_s) {
    return extrapolated.pre_extrapolation === "linear"
      ? linearExtrapolatedValue(keyframes, localTimeS, true) ?? keyframes[0].value
      : keyframes[0].value;
  }

  for (let index = 0; index < keyframes.length - 1; index += 1) {
    const current = keyframes[index];
    const next = keyframes[index + 1];
    if (localTimeS > next.time_s) continue;
    if (current.interpolation === "hold" || next.time_s <= current.time_s) {
      return current.value;
    }
    const raw = (localTimeS - current.time_s) / (next.time_s - current.time_s);
    if (current.interpolation === "step") {
      return raw < 0.5 ? current.value : next.value;
    }
    if (current.interpolation === "spring") {
      const spring = (current as KeyframeWithSpring).spring ?? {
        mass: 1,
        stiffness: 100,
        damping: 12,
      };
      const sprung = springProgress(raw, spring);
      return current.value + (next.value - current.value) * sprung;
    }
    const bezier = (current as KeyframeWithBezier).bezier;
    const eased =
      current.interpolation === "bezier" && bezier
        ? bezierProgress(raw, effectiveBezierHandles(current, next, bezier))
        : easeProgress(raw, current.easing);
    return current.value + (next.value - current.value) * eased;
  }

  const last = keyframes[keyframes.length - 1];
  return extrapolated.post_extrapolation === "linear"
    ? linearExtrapolatedValue(keyframes, localTimeS, false) ?? last.value
    : last.value;
}

export function evaluateAnimations(
  animations: TimelineParameterAnimation[] | undefined,
  localTimeS: number,
): AnimationValues {
  const values: AnimationValues = {};
  for (const animation of animations ?? []) {
    if (!isPhase3AParameter(animation.target.parameter)) continue;
    if (animation.target.parameter.endsWith(".position")) {
      const point = evaluateMotionPath(animation, localTimeS);
      if (point === null) continue;
      const prefix = animation.target.parameter.replace(/\.position$/, "");
      values[`${prefix}.x`] = point.x;
      values[`${prefix}.y`] = point.y;
      continue;
    }
    const value = evaluateAnimationValue(animation, localTimeS);
    if (value === null || !Number.isFinite(value)) continue;
    values[animation.target.parameter] = value;
  }
  return values;
}

function evaluateMotionPath(
  animation: TimelineParameterAnimation,
  localTimeS: number,
): { x: number; y: number } | null {
  const points = animation.motion_path?.points ?? [];
  if (points.length === 0) return null;
  if (localTimeS <= points[0].time_s) return { x: points[0].x, y: points[0].y };
  for (let index = 0; index < points.length - 1; index += 1) {
    const current = points[index];
    const next = points[index + 1];
    if (localTimeS > next.time_s) continue;
    if (next.time_s <= current.time_s) return { x: current.x, y: current.y };
    const progress = (localTimeS - current.time_s) / (next.time_s - current.time_s);
    return {
      x: current.x + (next.x - current.x) * progress,
      y: current.y + (next.y - current.y) * progress,
    };
  }
  const last = points[points.length - 1];
  return { x: last.x, y: last.y };
}

export function clampOpacity(value: number): number {
  return Math.max(0, Math.min(1, value));
}

function linearExtrapolatedValue(
  keyframes: TimelineParameterAnimation["keyframes"],
  localTimeS: number,
  beforeFirst: boolean,
): number | null {
  if (keyframes.length < 2) return null;
  const a = beforeFirst ? keyframes[0] : keyframes[keyframes.length - 2];
  const b = beforeFirst ? keyframes[1] : keyframes[keyframes.length - 1];
  const dt = b.time_s - a.time_s;
  if (dt <= 0) return null;
  const slope = (b.value - a.value) / dt;
  return beforeFirst
    ? a.value + slope * (localTimeS - a.time_s)
    : b.value + slope * (localTimeS - b.time_s);
}

function easeProgress(progress: number, easing: string): number {
  const p = Math.max(0, Math.min(1, progress));
  switch (easing) {
    case "ease_in_sine":
      return 1 - Math.cos((p * Math.PI) / 2);
    case "ease_out_sine":
      return Math.sin((p * Math.PI) / 2);
    case "ease_in_out_sine":
      return -(Math.cos(Math.PI * p) - 1) / 2;
    case "ease_in":
      return p * p;
    case "ease_out":
      return 1 - (1 - p) * (1 - p);
    case "ease_in_out":
      return p < 0.5 ? 2 * p * p : 1 - Math.pow(-2 * p + 2, 2) / 2;
    case "ease_in_cubic":
      return p ** 3;
    case "ease_out_cubic":
      return 1 - (1 - p) ** 3;
    case "ease_in_out_cubic":
      return p < 0.5 ? 4 * p ** 3 : 1 - Math.pow(-2 * p + 2, 3) / 2;
    case "ease_in_quart":
      return p ** 4;
    case "ease_out_quart":
      return 1 - (1 - p) ** 4;
    case "ease_in_out_quart":
      return p < 0.5 ? 8 * p ** 4 : 1 - Math.pow(-2 * p + 2, 4) / 2;
    case "ease_in_quint":
      return p ** 5;
    case "ease_out_quint":
      return 1 - (1 - p) ** 5;
    case "ease_in_out_quint":
      return p < 0.5 ? 16 * p ** 5 : 1 - Math.pow(-2 * p + 2, 5) / 2;
    case "ease_in_expo":
      return p === 0 ? 0 : 2 ** (10 * p - 10);
    case "ease_out_expo":
      return p === 1 ? 1 : 1 - 2 ** (-10 * p);
    case "ease_in_out_expo":
      return p === 0
        ? 0
        : p === 1
          ? 1
          : p < 0.5
            ? 2 ** (20 * p - 10) / 2
            : (2 - 2 ** (-20 * p + 10)) / 2;
    case "ease_in_circ":
      return 1 - Math.sqrt(1 - p ** 2);
    case "ease_out_circ":
      return Math.sqrt(1 - (p - 1) ** 2);
    case "ease_in_out_circ":
      return p < 0.5
        ? (1 - Math.sqrt(1 - (2 * p) ** 2)) / 2
        : (Math.sqrt(1 - (-2 * p + 2) ** 2) + 1) / 2;
    case "ease_in_back":
      return easeInBack(p);
    case "ease_out_back":
      return easeOutBack(p);
    case "ease_in_out_back":
      return easeInOutBack(p);
    case "ease_in_elastic":
      return easeInElastic(p);
    case "ease_out_elastic":
      return easeOutElastic(p);
    case "ease_in_out_elastic":
      return easeInOutElastic(p);
    case "ease_in_bounce":
      return 1 - easeOutBounce(1 - p);
    case "ease_out_bounce":
      return easeOutBounce(p);
    case "ease_in_out_bounce":
      return p < 0.5
        ? (1 - easeOutBounce(1 - 2 * p)) / 2
        : (1 + easeOutBounce(2 * p - 1)) / 2;
    default:
      return p;
  }
}

function easeInBack(p: number): number {
  const c1 = 1.70158;
  const c3 = c1 + 1;
  return c3 * p ** 3 - c1 * p ** 2;
}

function easeOutBack(p: number): number {
  const c1 = 1.70158;
  const c3 = c1 + 1;
  return 1 + c3 * (p - 1) ** 3 + c1 * (p - 1) ** 2;
}

function easeInOutBack(p: number): number {
  const c1 = 1.70158;
  const c2 = c1 * 1.525;
  return p < 0.5
    ? ((2 * p) ** 2 * ((c2 + 1) * 2 * p - c2)) / 2
    : ((2 * p - 2) ** 2 * ((c2 + 1) * (p * 2 - 2) + c2) + 2) / 2;
}

function easeInElastic(p: number): number {
  if (p === 0) return 0;
  if (p === 1) return 1;
  const c4 = (2 * Math.PI) / 3;
  return -(2 ** (10 * p - 10)) * Math.sin((p * 10 - 10.75) * c4);
}

function easeOutElastic(p: number): number {
  if (p === 0) return 0;
  if (p === 1) return 1;
  const c4 = (2 * Math.PI) / 3;
  return 2 ** (-10 * p) * Math.sin((p * 10 - 0.75) * c4) + 1;
}

function easeInOutElastic(p: number): number {
  if (p === 0) return 0;
  if (p === 1) return 1;
  const c5 = (2 * Math.PI) / 4.5;
  return p < 0.5
    ? -(2 ** (20 * p - 10) * Math.sin((20 * p - 11.125) * c5)) / 2
    : (2 ** (-20 * p + 10) * Math.sin((20 * p - 11.125) * c5)) / 2 + 1;
}

function easeOutBounce(p: number): number {
  const n1 = 7.5625;
  const d1 = 2.75;
  if (p < 1 / d1) return n1 * p ** 2;
  if (p < 2 / d1) {
    const shifted = p - 1.5 / d1;
    return n1 * shifted ** 2 + 0.75;
  }
  if (p < 2.5 / d1) {
    const shifted = p - 2.25 / d1;
    return n1 * shifted ** 2 + 0.9375;
  }
  const shifted = p - 2.625 / d1;
  return n1 * shifted ** 2 + 0.984375;
}

function springProgress(progress: number, spring: SpringParameters): number {
  if (progress <= 0) return 0;
  if (progress >= 1) return 1;
  const mass = Math.max(Number.EPSILON, spring.mass);
  const stiffness = Math.max(Number.EPSILON, spring.stiffness);
  const damping = Math.max(0, spring.damping);
  const p = Math.max(0, progress);
  const omega0 = Math.sqrt(stiffness / mass);
  const zeta = damping / (2 * Math.sqrt(stiffness * mass));
  if (zeta < 1 - 1e-6) {
    const omegaD = omega0 * Math.sqrt(1 - zeta * zeta);
    const envelope = Math.exp(-zeta * omega0 * p);
    return (
      1 -
      envelope *
        (Math.cos(omegaD * p) +
          (zeta / Math.sqrt(1 - zeta * zeta)) * Math.sin(omegaD * p))
    );
  }
  return 1 - Math.exp(-omega0 * p) * (1 + omega0 * p);
}

function bezierProgress(progress: number, handles: BezierHandles): number {
  const p = Math.max(0, Math.min(1, progress));
  let t = p;
  for (let index = 0; index < 8; index += 1) {
    const x = cubicBezier(0, handles.out_x, handles.in_x, 1, t) - p;
    const slope = cubicBezierDerivative(handles.out_x, handles.in_x, t);
    if (Math.abs(x) < 1e-12 || Math.abs(slope) < 1e-12) break;
    t = Math.max(0, Math.min(1, t - x / slope));
  }
  return cubicBezier(0, handles.out_y, handles.in_y, 1, t);
}

function effectiveBezierHandles(
  current: TimelineParameterAnimation["keyframes"][number],
  next: TimelineParameterAnimation["keyframes"][number],
  handles: BezierHandles,
): BezierHandles {
  return {
    ...handles,
    out_y: current.tangent_mode === "flat" ? 0 : handles.out_y,
    in_y: next.tangent_mode === "flat" ? 1 : handles.in_y,
  };
}

function cubicBezier(p0: number, p1: number, p2: number, p3: number, t: number): number {
  const inv = 1 - t;
  return inv ** 3 * p0 + 3 * inv ** 2 * t * p1 + 3 * inv * t ** 2 * p2 + t ** 3 * p3;
}

function cubicBezierDerivative(p1: number, p2: number, t: number): number {
  const inv = 1 - t;
  return 3 * inv ** 2 * p1 + 6 * inv * t * (p2 - p1) + 3 * t ** 2 * (1 - p2);
}

export const evaluateAnimationValueForTest = evaluateAnimationValue;
