import { invoke } from "@tauri-apps/api/core";
import type { TimelineParameterAnimation } from "../protocol";
import type { TimelineItem } from "../timeline/store";
import {
  serializeEdl,
  type EdlParameterAnimation,
} from "../timeline/edlBuilder";

const MOTION_CURVE_PRESETS = [
  "Linear",
  "Ease In/Out",
  "Hold",
  "Spring",
  "Smooth Drift",
] as const;

type MotionCurvePreset = (typeof MOTION_CURVE_PRESETS)[number];

export function MotionAnimationControl({
  clip,
}: {
  clip: Extract<TimelineItem, { kind: "clip" }>;
}) {
  const animations = clip.animations ?? [];
  const canAuthorTitlePath = clip.title !== null;

  function proposeAnimation(animation: EdlParameterAnimation) {
    invoke<string>("propose_user_edit", {
      edlText: serializeEdl([{ kind: "set_parameter_animation", animation }]),
    }).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (parameter animation) failed", err);
    });
  }

  function addDriftPath() {
    if (!canAuthorTitlePath) return;
    const duration = Math.max(0.25, clip.duration_s);
    const animation: EdlParameterAnimation = {
      id: `title-drift-${clip.clip_uuid}`,
      target: {
        kind: "clip_parameter",
        clip_id: clip.clip_uuid,
        parameter: "title.position",
      },
      keyframes: [
        {
          time_s: 0,
          value: 0,
          interpolation: "bezier",
          easing: "ease_in_out",
          bezier: {
            out_x: 0.22,
            out_y: 0,
            in_x: 0.78,
            in_y: 1,
          },
          tangent_mode: "aligned",
          spring: null,
        },
        {
          time_s: duration,
          value: 1,
          interpolation: "linear",
          easing: "linear",
          bezier: null,
          tangent_mode: "auto",
          spring: null,
        },
      ],
      pre_extrapolation: "hold",
      post_extrapolation: "hold",
      motion_path: {
        points: [
          {
            time_s: 0,
            x: -0.035,
            y: 0.015,
            outgoing_control: { x: -0.012, y: 0.03 },
          },
          {
            time_s: duration,
            x: 0.035,
            y: -0.015,
            incoming_control: { x: 0.012, y: -0.03 },
          },
        ],
      },
      rationale: "Add a subtle title drift motion path.",
    };
    proposeAnimation(animation);
  }

  function applyPreset(animation: TimelineParameterAnimation, preset: MotionCurvePreset) {
    proposeAnimation(animationWithPreset(animation, clip.duration_s, preset));
  }

  return (
    <div className="properties-motion">
      {animations.length === 0 ? (
        <div className="properties-intent-meta">No parameter animations on this clip.</div>
      ) : (
        <div className="properties-motion-list">
          {animations.map((animation) => (
            <div
              className="properties-motion-path-summary"
              key={animation.id}
              title={animation.id}
            >
              <span className="properties-motion-target">
                {animation.target.parameter}
              </span>
              <span className="properties-intent-meta">
                {motionSummary(animation)}
              </span>
              <MotionCurvePreview animation={animation} />
              <div className="properties-motion-meta-grid">
                <span>Curve {curveModeSummary(animation)}</span>
                <span>Velocity {velocitySummary(animation)}</span>
              </div>
              <div className="properties-motion-preset-row">
                {MOTION_CURVE_PRESETS.map((preset) => (
                  <button
                    className="properties-motion-preset"
                    key={preset}
                    type="button"
                    onClick={() => applyPreset(animation, preset)}
                  >
                    {preset}
                  </button>
                ))}
              </div>
            </div>
          ))}
        </div>
      )}
      {canAuthorTitlePath && (
        <div className="properties-action-row properties-motion-actions">
          <button className="properties-apply" type="button" onClick={addDriftPath}>
            Add drift path
          </button>
          <span className="properties-action-hint">
            Bezier keyframes with a curved title.position motion path
          </span>
        </div>
      )}
    </div>
  );
}

function animationWithPreset(
  animation: TimelineParameterAnimation,
  clipDurationS: number,
  preset: MotionCurvePreset,
): EdlParameterAnimation {
  const keyframes = normalizedPresetKeyframes(animation, preset);
  return {
    ...animation,
    target: {
      kind: "clip_parameter",
      clip_id: animation.target.clip_id,
      parameter: animation.target.parameter,
    },
    keyframes,
    pre_extrapolation: "hold",
    post_extrapolation: preset === "Spring" ? "hold" : animation.post_extrapolation,
    motion_path:
      preset === "Smooth Drift" && animation.target.parameter.endsWith(".position")
        ? smoothDriftPath(animation, clipDurationS)
        : animation.motion_path,
    metadata_only: false,
    rationale: `Apply ${preset} curve preset to ${animation.target.parameter}.`,
  };
}

function normalizedPresetKeyframes(
  animation: TimelineParameterAnimation,
  preset: MotionCurvePreset,
): TimelineParameterAnimation["keyframes"] {
  const keyframes =
    animation.keyframes.length >= 2
      ? animation.keyframes
      : [
          {
            time_s: 0,
            value: 0,
            interpolation: "linear",
            easing: "linear",
            bezier: null,
            tangent_mode: "auto",
            spring: null,
          },
          {
            time_s: 1,
            value: 1,
            interpolation: "linear",
            easing: "linear",
            bezier: null,
            tangent_mode: "auto",
            spring: null,
          },
        ];
  return keyframes.map((keyframe, index) => {
    const isLast = index === keyframes.length - 1;
    if (isLast) {
      return {
        ...keyframe,
        interpolation: "linear",
        easing: "linear",
        bezier: null,
        tangent_mode: "auto",
        spring: null,
      };
    }
    switch (preset) {
      case "Ease In/Out":
      case "Smooth Drift":
        return {
          ...keyframe,
          interpolation: "bezier",
          easing: "ease_in_out",
          bezier: { out_x: 0.22, out_y: 0, in_x: 0.78, in_y: 1 },
          tangent_mode: "aligned",
          spring: null,
        };
      case "Hold":
        return {
          ...keyframe,
          interpolation: "hold",
          easing: "linear",
          bezier: null,
          tangent_mode: "flat",
          spring: null,
        };
      case "Spring":
        return {
          ...keyframe,
          interpolation: "spring",
          easing: "linear",
          bezier: null,
          tangent_mode: "auto",
          spring: { mass: 1, stiffness: 100, damping: 12 },
        };
      case "Linear":
        return {
          ...keyframe,
          interpolation: "linear",
          easing: "linear",
          bezier: null,
          tangent_mode: "auto",
          spring: null,
        };
    }
  });
}

function smoothDriftPath(
  animation: TimelineParameterAnimation,
  clipDurationS: number,
): TimelineParameterAnimation["motion_path"] {
  const duration = Math.max(0.25, clipDurationS);
  const first = animation.motion_path?.points[0] ?? { time_s: 0, x: -0.035, y: 0.015 };
  const existingPoints = animation.motion_path?.points ?? [];
  const last = existingPoints[existingPoints.length - 1] ?? {
    time_s: duration,
    x: 0.035,
    y: -0.015,
  };
  return {
    points: [
      {
        time_s: first.time_s,
        x: first.x,
        y: first.y,
        outgoing_control: {
          x: first.x + (last.x - first.x) * 0.33,
          y: first.y,
        },
      },
      {
        time_s: last.time_s,
        x: last.x,
        y: last.y,
        incoming_control: {
          x: first.x + (last.x - first.x) * 0.67,
          y: last.y,
        },
      },
    ],
  };
}

function motionSummary(animation: TimelineParameterAnimation): string {
  const keyframes = animation.keyframes.length;
  const bezier = animation.keyframes.filter((keyframe) => keyframe.bezier !== null).length;
  const pathPoints = animation.motion_path?.points.length ?? 0;
  const pathHandles =
    animation.motion_path?.points.filter(
      (point) => point.outgoing_control || point.incoming_control,
    ).length ?? 0;
  const parts = [
    `${keyframes} keyframe${keyframes === 1 ? "" : "s"}`,
    bezier > 0 ? `${bezier} Bezier` : null,
    pathPoints > 0 ? `${pathPoints} path point${pathPoints === 1 ? "" : "s"}` : null,
    pathHandles > 0 ? `${pathHandles} path handle${pathHandles === 1 ? "" : "s"}` : null,
  ].filter(Boolean);
  return parts.join(" · ");
}

function curveModeSummary(animation: TimelineParameterAnimation): string {
  const modes = new Set(animation.keyframes.map((keyframe) => keyframe.interpolation));
  if (modes.size === 0) return "path";
  return Array.from(modes).join("/");
}

function velocitySummary(animation: TimelineParameterAnimation): string {
  const keyframes = animation.keyframes;
  if (keyframes.length < 2) return animation.motion_path ? "path only" : "constant";
  const first = keyframes[0];
  const last = keyframes[keyframes.length - 1];
  const duration = Math.max(0.001, last.time_s - first.time_s);
  const velocity = (last.value - first.value) / duration;
  if (!Number.isFinite(velocity)) return "n/a";
  return `${formatPanelNumber(velocity)}/s`;
}

function MotionCurvePreview({ animation }: { animation: TimelineParameterAnimation }) {
  const samples = curvePreviewSamples(animation);
  const path = samples
    .map((point, index) => `${index === 0 ? "M" : "L"} ${point.x} ${point.y}`)
    .join(" ");
  return (
    <svg
      aria-label={`${animation.target.parameter} curve preview`}
      className="properties-motion-curve-preview"
      focusable="false"
      preserveAspectRatio="none"
      role="img"
      viewBox="0 0 120 44"
    >
      <line className="properties-motion-grid-line" x1="0" x2="120" y1="22" y2="22" />
      <path className="properties-motion-curve-line" d={path} />
      {samples
        .filter((_, index) => index === 0 || index === samples.length - 1)
        .map((point) => (
          <circle
            className="properties-motion-curve-point"
            cx={point.x}
            cy={point.y}
            key={`${point.x}-${point.y}`}
            r="2.4"
          />
        ))}
    </svg>
  );
}

function curvePreviewSamples(animation: TimelineParameterAnimation): Array<{ x: number; y: number }> {
  const keyframes = animation.keyframes;
  if (keyframes.length === 0) {
    return [
      { x: 0, y: 22 },
      { x: 120, y: 22 },
    ];
  }
  const first = keyframes[0];
  const last = keyframes[keyframes.length - 1];
  const minValue = Math.min(...keyframes.map((keyframe) => keyframe.value));
  const maxValue = Math.max(...keyframes.map((keyframe) => keyframe.value));
  const duration = Math.max(0.001, last.time_s - first.time_s);
  const valueRange = Math.max(0.001, maxValue - minValue);
  return Array.from({ length: 25 }, (_, index) => {
    const progress = index / 24;
    const timeS = first.time_s + duration * progress;
    const value = evaluatePreviewValue(keyframes, timeS);
    return {
      x: progress * 120,
      y: 40 - ((value - minValue) / valueRange) * 36,
    };
  });
}

function evaluatePreviewValue(
  keyframes: TimelineParameterAnimation["keyframes"],
  timeS: number,
): number {
  if (timeS <= keyframes[0].time_s) return keyframes[0].value;
  for (let index = 0; index < keyframes.length - 1; index += 1) {
    const current = keyframes[index];
    const next = keyframes[index + 1];
    if (timeS > next.time_s) continue;
    if (next.time_s <= current.time_s || current.interpolation === "hold") {
      return current.value;
    }
    const raw = (timeS - current.time_s) / (next.time_s - current.time_s);
    const eased =
      current.interpolation === "bezier" && current.bezier
        ? cubicBezierY(raw, current.bezier.out_y, current.bezier.in_y)
        : raw;
    return current.value + (next.value - current.value) * eased;
  }
  return keyframes[keyframes.length - 1].value;
}

function cubicBezierY(progress: number, outY: number, inY: number): number {
  const inverse = 1 - progress;
  return 3 * inverse ** 2 * progress * outY + 3 * inverse * progress ** 2 * inY + progress ** 3;
}

function formatPanelNumber(value: number): string {
  return Number.isInteger(value) ? value.toString() : value.toFixed(2);
}
