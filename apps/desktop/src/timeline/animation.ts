import type { TimelineParameterAnimation } from "../protocol";

export type AnimationValues = Record<string, number>;

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
    const eased = easeProgress(raw, current.easing);
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

export const evaluateAnimationValueForTest = evaluateAnimationValue;
