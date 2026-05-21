import type { CSSProperties } from "react";
import type { TimelineParameterAnimation } from "../protocol";
import { clampOpacity, evaluateAnimations } from "../timeline/animation.js";

export const TIMELINE_RENDER_HEIGHT_PX = 1080;

type VideoOverlayStyleInput = {
  mode: string;
  corner: "top_left" | "top_right" | "bottom_left" | "bottom_right";
  scale: number;
  marginPct: number;
  zIndex: number;
  timelineStart: number;
  previewHeightPx?: number | null;
  animations: TimelineParameterAnimation[];
};

export function videoOverlayStyle(
  overlay: VideoOverlayStyleInput,
  timelineTime: number,
): CSSProperties {
  const zIndex = 10 + overlay.zIndex;
  const animated = evaluateAnimations(
    overlay.animations,
    timelineTime - overlay.timelineStart,
  );
  const opacity = clampOpacity(animated["overlay.opacity"] ?? 1);
  const xOffset = animated["overlay.x"] ?? 0;
  const yOffset = animated["overlay.y"] ?? 0;
  const scaleMultiplier = animated["overlay.scale"] ?? 1;
  const rotationDeg = animated["overlay.rotation_deg"] ?? 0;
  const blurPx = scaledPreviewBlurPx(
    Math.max(0, animated["overlay.blur"] ?? 0),
    overlay.previewHeightPx,
  );
  const translateTransform = `translate(${xOffset * 100}vw, ${yOffset * 100}vh)`;
  const scaleTransform = `scale(${scaleMultiplier})`;
  const rotateTransform = `rotate(${rotationDeg}deg)`;
  if (overlay.mode === "full_frame") {
    return {
      position: "absolute",
      inset: 0,
      width: "100%",
      height: "100%",
      objectFit: "contain",
      opacity,
      filter: blurPx > 0 ? `blur(${blurPx}px)` : undefined,
      transform: `${translateTransform} ${scaleTransform} ${rotateTransform}`,
      transformOrigin: "center center",
      zIndex,
    };
  }
  const size = `${overlay.scale * 100}%`;
  const margin = `${overlay.marginPct * 100}%`;
  return {
    position: "absolute",
    width: size,
    height: "auto",
    maxHeight: `calc(100% - (${margin} * 2))`,
    objectFit: "contain",
    borderRadius: 6,
    boxShadow: "0 10px 28px rgba(0, 0, 0, 0.42)",
    opacity,
    filter: blurPx > 0 ? `blur(${blurPx}px)` : undefined,
    transform: `${translateTransform} ${scaleTransform} ${rotateTransform}`,
    transformOrigin: "center center",
    zIndex,
    ...cornerStyle(overlay.corner, margin),
  };
}

function scaledPreviewBlurPx(renderBlurPx: number, previewHeightPx?: number | null): number {
  if (!Number.isFinite(renderBlurPx) || renderBlurPx <= 0) return 0;
  if (!previewHeightPx || !Number.isFinite(previewHeightPx) || previewHeightPx <= 0) {
    return renderBlurPx;
  }
  return renderBlurPx * (previewHeightPx / TIMELINE_RENDER_HEIGHT_PX);
}

function cornerStyle(
  corner: VideoOverlayStyleInput["corner"],
  margin: string,
): CSSProperties {
  switch (corner) {
    case "top_left":
      return { top: margin, left: margin };
    case "top_right":
      return { top: margin, right: margin };
    case "bottom_left":
      return { bottom: margin, left: margin };
    case "bottom_right":
      return { bottom: margin, right: margin };
  }
}
