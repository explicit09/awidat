import { convertFileSrc } from "@tauri-apps/api/core";
import type { TimelineParameterAnimation } from "../../protocol";
import { clampOpacity, evaluateAnimations } from "../../timeline/animation";
import type { TimelineSnapshot } from "../../timeline/store";
import {
  titleAnimation,
  titleOverlayBox,
  titleOverlayStyle,
  titlePosition,
  titleReveal,
  titleRevealText,
  type PreviewTitleOverlay,
} from "./titles";

export function projectAssetUrl(projectRoot: string | null, relPath: string | null): string | null {
  if (!projectRoot || !relPath) return null;
  if (relPath.startsWith("/") || relPath.includes("..")) return null;
  const root = projectRoot.endsWith("/") ? projectRoot.slice(0, -1) : projectRoot;
  return convertFileSrc(`${root}/${relPath}`);
}

export type PreviewMotionShapeOverlay = {
  key: string;
  startS: number;
  endS: number;
  shape: "rect";
  x: number;
  y: number;
  width: number;
  height: number;
  color: string;
  opacity: number;
  scale: number;
  anchorX: number;
  anchorY: number;
  rotationDeg: number;
  animations: TimelineParameterAnimation[];
};

export type PreviewMotionImageOverlay = {
  key: string;
  startS: number;
  endS: number;
  src: string;
  x: number;
  y: number;
  width: number;
  height: number;
  opacity: number;
  fit: "cover" | "contain" | "fill";
  scale: number;
  anchorX: number;
  anchorY: number;
  rotationDeg: number;
  animations: TimelineParameterAnimation[];
};

export type PreviewMotionSceneOverlay =
  | { kind: "title"; overlay: PreviewTitleOverlay }
  | { kind: "shape"; overlay: PreviewMotionShapeOverlay }
  | { kind: "image"; overlay: PreviewMotionImageOverlay };

export function activeMotionSceneOverlays(
  snapshot: TimelineSnapshot,
  projectRoot: string | null,
): PreviewMotionSceneOverlay[] {
  const overlays: PreviewMotionSceneOverlay[] = [];
  for (const track of snapshot.tracks) {
    for (const item of track.items) {
      if (item.kind !== "clip") continue;
      const startS = item.track_start_s;
      const endS = item.track_start_s + item.duration_s;
      if (!Number.isFinite(startS) || !Number.isFinite(endS) || endS <= startS) {
        continue;
      }
      if (item.title?.role === "motion_scene") {
        overlays.push({
          kind: "title",
          overlay: {
            key: item.clip_uuid || item.name,
            startS,
            endS,
            text: item.title.text,
            position: titlePosition(item.title.position),
            fontSize: item.title.font_size,
            color: item.title.color || "#FFFFFF",
            fontWeight: item.title.font_weight === "bold" ? "bold" : "normal",
            animation: titleAnimation(item.title.animation),
            reveal: titleReveal(item.title.reveal),
            animations: item.animations ?? [],
            isMotionScene: true,
            box: titleOverlayBox(item.title),
          },
        });
      }
      if (item.motion_shape !== null && item.motion_shape.shape === "rect") {
        overlays.push({
          kind: "shape",
          overlay: {
            key: item.clip_uuid || item.name,
            startS,
            endS,
            shape: "rect",
            x: item.motion_shape.x,
            y: item.motion_shape.y,
            width: item.motion_shape.width,
            height: item.motion_shape.height,
            color: item.motion_shape.color || "#FFFFFF",
            opacity: clampOpacity(item.motion_shape.opacity),
            scale: item.motion_shape.scale,
            anchorX: item.motion_shape.anchor_x,
            anchorY: item.motion_shape.anchor_y,
            rotationDeg: item.motion_shape.rotation_deg,
            animations: item.animations ?? [],
          },
        });
      }
      if (item.motion_image !== null) {
        const src = projectAssetUrl(projectRoot, item.motion_image.asset_id);
        if (src === null) continue;
        overlays.push({
          kind: "image",
          overlay: {
            key: item.clip_uuid || item.name,
            startS,
            endS,
            src,
            x: item.motion_image.x,
            y: item.motion_image.y,
            width: item.motion_image.width,
            height: item.motion_image.height,
            opacity: clampOpacity(item.motion_image.opacity),
            fit: motionImageFit(item.motion_image.fit),
            scale: item.motion_image.scale,
            anchorX: item.motion_image.anchor_x,
            anchorY: item.motion_image.anchor_y,
            rotationDeg: item.motion_image.rotation_deg,
            animations: item.animations ?? [],
          },
        });
      }
    }
  }
  return overlays;
}

export function TimelineMotionSceneOverlays({
  overlays,
  timelineTime,
}: {
  overlays: PreviewMotionSceneOverlay[];
  timelineTime: number;
}) {
  const active = overlays.filter(
    ({ overlay }) => timelineTime >= overlay.startS && timelineTime < overlay.endS,
  );
  if (active.length === 0) return null;
  return (
    <div className="timeline-motion-scene-layer" aria-hidden="true">
      {active.map((layer) => {
        if (layer.kind === "title") {
          const overlay = layer.overlay;
          return (
            <div
              key={`title:${overlay.key}`}
              className={
                overlay.box
                  ? "timeline-title-overlay"
                  : `timeline-title-overlay title-pos-${overlay.position}`
              }
              style={titleOverlayStyle(overlay, timelineTime)}
            >
              {titleRevealText(overlay, timelineTime)}
            </div>
          );
        }
        if (layer.kind === "shape") {
          const overlay = layer.overlay;
          return (
            <div
              key={`shape:${overlay.key}`}
              className="timeline-motion-shape-rect"
              style={motionShapeOverlayStyle(overlay, timelineTime)}
            />
          );
        }
        const overlay = layer.overlay;
        return (
          <img
            key={`image:${overlay.key}`}
            className="timeline-motion-image"
            src={overlay.src}
            style={motionImageOverlayStyle(overlay, timelineTime)}
          />
        );
      })}
    </div>
  );
}

export function motionShapeOverlayStyle(
  overlay: PreviewMotionShapeOverlay,
  timelineTime: number,
): React.CSSProperties {
  const animated = evaluateAnimations(overlay.animations, timelineTime - overlay.startS);
  const x = animated["overlay.x"] ?? overlay.x;
  const y = animated["overlay.y"] ?? overlay.y;
  const scale = animated["overlay.scale"] ?? overlay.scale;
  const rotationDeg = animated["overlay.rotation_deg"] ?? overlay.rotationDeg;
  const opacity = clampOpacity(animated["overlay.opacity"] ?? overlay.opacity);
  return {
    left: `${x * 100}%`,
    top: `${y * 100}%`,
    width: `${overlay.width * 100}%`,
    height: `${overlay.height * 100}%`,
    background: overlay.color,
    opacity,
    transform: `scale(${scale}) rotate(${rotationDeg}deg)`,
    transformOrigin: `${overlay.anchorX * 100}% ${overlay.anchorY * 100}%`,
  };
}

export function motionImageOverlayStyle(
  overlay: PreviewMotionImageOverlay,
  timelineTime: number,
): React.CSSProperties {
  const animated = evaluateAnimations(overlay.animations, timelineTime - overlay.startS);
  const x = animated["overlay.x"] ?? overlay.x;
  const y = animated["overlay.y"] ?? overlay.y;
  const scale = animated["overlay.scale"] ?? overlay.scale;
  const rotationDeg = animated["overlay.rotation_deg"] ?? overlay.rotationDeg;
  const opacity = clampOpacity(animated["overlay.opacity"] ?? overlay.opacity);
  return {
    left: `${x * 100}%`,
    top: `${y * 100}%`,
    width: `${overlay.width * 100}%`,
    height: `${overlay.height * 100}%`,
    opacity,
    objectFit: overlay.fit,
    transform: `scale(${scale}) rotate(${rotationDeg}deg)`,
    transformOrigin: `${overlay.anchorX * 100}% ${overlay.anchorY * 100}%`,
  };
}

export function motionImageFit(value: string): "cover" | "contain" | "fill" {
  if (value === "contain") return "contain";
  if (value === "stretch") return "fill";
  return "cover";
}
