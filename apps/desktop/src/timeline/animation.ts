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

const PHASE_3A_PARAMETERS = new Set([
  "title.opacity",
  "title.x",
  "title.y",
  "overlay.opacity",
  "overlay.x",
  "overlay.y",
  "overlay.scale",
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
  if (localTimeS <= keyframes[0].time_s) return keyframes[0].value;

  for (let index = 0; index < keyframes.length - 1; index += 1) {
    const current = keyframes[index];
    const next = keyframes[index + 1];
    if (localTimeS > next.time_s) continue;
    if (current.interpolation === "hold" || next.time_s <= current.time_s) {
      return current.value;
    }
    const raw = (localTimeS - current.time_s) / (next.time_s - current.time_s);
    const bezier = (current as KeyframeWithBezier).bezier;
    const eased =
      current.interpolation === "bezier" && bezier
        ? bezierProgress(raw, bezier)
        : easeProgress(raw, current.easing);
    return current.value + (next.value - current.value) * eased;
  }

  return keyframes[keyframes.length - 1].value;
}

export function evaluateAnimations(
  animations: TimelineParameterAnimation[] | undefined,
  localTimeS: number,
): AnimationValues {
  const values: AnimationValues = {};
  for (const animation of animations ?? []) {
    if (!isPhase3AParameter(animation.target.parameter)) continue;
    const value = evaluateAnimationValue(animation, localTimeS);
    if (value === null || !Number.isFinite(value)) continue;
    values[animation.target.parameter] = value;
  }
  return values;
}

export function clampOpacity(value: number): number {
  return Math.max(0, Math.min(1, value));
}

function easeProgress(progress: number, easing: string): number {
  const p = Math.max(0, Math.min(1, progress));
  switch (easing) {
    case "ease_in":
      return p * p;
    case "ease_out":
      return 1 - (1 - p) * (1 - p);
    case "ease_in_out":
      return p < 0.5 ? 2 * p * p : 1 - Math.pow(-2 * p + 2, 2) / 2;
    default:
      return p;
  }
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

function cubicBezier(p0: number, p1: number, p2: number, p3: number, t: number): number {
  const inv = 1 - t;
  return inv ** 3 * p0 + 3 * inv ** 2 * t * p1 + 3 * inv * t ** 2 * p2 + t ** 3 * p3;
}

function cubicBezierDerivative(p1: number, p2: number, t: number): number {
  const inv = 1 - t;
  return 3 * inv ** 2 * p1 + 6 * inv * t * (p2 - p1) + 3 * t ** 2 * (1 - p2);
}

export const evaluateAnimationValueForTest = evaluateAnimationValue;
