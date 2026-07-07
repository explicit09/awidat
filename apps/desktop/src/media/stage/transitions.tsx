import { useEffect, useRef, useState, type CSSProperties } from "react";
import { useMediaStore } from "../store";
import { cachedMediaStreamUrl, mediaStreamUrl } from "../mediaStreamUrl";
import { useGpuTransitionPreview } from "../useGpuTransitionPreview";
import {
  type PreviewTransition,
  safeSegmentSpeed,
  sourceTimeForTimelineTime,
} from "../../timeline/usePlaySegments";
import { tryAssignCurrentTime } from "../SegmentedVideoView";

export function TimelineTransitionOverlay({
  transition,
  timelineTime,
  isPlaying,
}: {
  transition: PreviewTransition | null;
  timelineTime: number;
  isPlaying: boolean;
}) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const lastSyncKeyRef = useRef<string>("");
  const [src, setSrc] = useState<string | null>(null);
  const setMediaError = useMediaStore((s) => s.setMediaError);
  const overlaySide =
    transition && timelineTime < transition.cutTime ? "incoming" : "outgoing";
  const overlaySegment =
    transition && overlaySide === "incoming" ? transition.to : transition?.from;

  useEffect(() => {
    if (!transition || !overlaySegment) {
      setSrc(null);
      return;
    }
    const cached = cachedMediaStreamUrl(overlaySegment.proxyPath);
    if (cached) {
      setSrc(cached);
      return;
    }
    let cancelled = false;
    mediaStreamUrl(overlaySegment.proxyPath)
      .then((url) => {
        if (!cancelled) setSrc(url);
      })
      .catch((e) => {
        if (!cancelled) {
          setMediaError(`Could not open transition preview media: ${String(e)}`);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [transition, overlaySegment, setMediaError]);

  useEffect(() => {
    const v = videoRef.current;
    if (!v || !transition || !overlaySegment || !src) return;
    const sourceTime = sourceTimeForTimelineTime(overlaySegment, timelineTime);
    const syncKey = `${overlaySegment.proxyPath}:${overlaySide}:${transition.timelineStart}`;
    const force = syncKey !== lastSyncKeyRef.current || !isPlaying || v.paused;
    lastSyncKeyRef.current = syncKey;
    const drift = Math.abs((v.currentTime || 0) - sourceTime);
    if (Number.isFinite(sourceTime) && drift > (force ? 0.02 : 0.5)) {
      tryAssignCurrentTime(v, sourceTime);
    }
    const speed = safeSegmentSpeed(overlaySegment.speed);
    if (Math.abs(v.playbackRate - speed) > 0.001) v.playbackRate = speed;
    if (isPlaying && v.paused) {
      v.play().catch(() => {});
    } else if (!isPlaying && !v.paused) {
      v.pause();
    }
  }, [transition, overlaySegment, overlaySide, src, timelineTime, isPlaying]);

  if (!transition || !overlaySegment || !src) return null;
  const progress = transitionProgress(transition, timelineTime);
  const visualStyle = transitionVisualStyle(transition, progress, overlaySide);

  return (
    <video
      ref={videoRef}
      className="video-el timeline-transition-preview"
      src={src}
      muted
      playsInline
      preload="auto"
      style={{
        opacity: transitionOpacity(transition.kind, progress, overlaySide),
        ...visualStyle,
        pointerEvents: "none",
        zIndex: 3,
      }}
      aria-hidden="true"
    />
  );
}

/**
 * GPU-rendered preview overlay. Shown when `transition.transitionId`
 * maps to a shader the GPU path renders better than the CSS overlay
 * can fake — keyframed Blur curves today, shader-only primitives
 * (Shake / ChromaticSplit / agent composites) later. The data URL
 * comes from the `render_transition_preview_frame` Tauri command and
 * sits on top of the video stack so it occludes both slot videos
 * during the transition window.
 */
export function GpuTransitionPreview({
  transition,
  timelineTime,
  width,
  height,
}: {
  transition: PreviewTransition | null;
  timelineTime: number;
  width: number;
  height: number;
}) {
  const dataUrl = useGpuTransitionPreview(transition, timelineTime, width, height);
  if (!dataUrl) return null;
  return (
    <img
      src={dataUrl}
      alt=""
      aria-hidden="true"
      style={{
        position: "absolute",
        inset: 0,
        width: "100%",
        height: "100%",
        objectFit: "contain",
        pointerEvents: "none",
        zIndex: 5,
      }}
    />
  );
}

export function TimelineTransitionColorOverlay({
  transition,
  timelineTime,
}: {
  transition: PreviewTransition | null;
  timelineTime: number;
}) {
  if (!transition || !isFlashWhite(transition.kind)) return null;
  const progress = transitionProgress(transition, timelineTime);
  const cutProgress =
    transition.duration <= 0
      ? 0.5
      : Math.max(0, Math.min(1, transition.inOffset / transition.duration));
  const falloff = Math.max(cutProgress, 1 - cutProgress, 0.001);
  const opacity = Math.max(0, 1 - Math.abs(progress - cutProgress) / falloff);
  return (
    <div
      className="timeline-transition-color-overlay"
      style={{
        background: "#fff",
        opacity,
        pointerEvents: "none",
        position: "absolute",
        inset: 0,
        zIndex: 4,
      }}
      aria-hidden="true"
    />
  );
}

export function transitionProgress(
  transition: PreviewTransition,
  timelineTime: number,
): number {
  if (transition.duration <= 0) return 1;
  return Math.max(
    0,
    Math.min(1, (timelineTime - transition.timelineStart) / transition.duration),
  );
}

export function baseTransitionOpacity(
  kind: string,
  progress: number,
  side: "outgoing" | "incoming",
): number {
  if (isFadeThroughBlack(kind)) {
    return side === "outgoing"
      ? Math.max(0, 1 - progress * 2)
      : Math.max(0, (progress - 0.5) * 2);
  }
  if (!isDissolveTransition(kind)) return 1;
  return side === "outgoing" ? 1 - progress : progress;
}

function transitionOpacity(
  kind: string,
  progress: number,
  side: "incoming" | "outgoing",
): number {
  if (isFadeThroughBlack(kind)) return 0;
  if (!isDissolveTransition(kind)) return 1;
  return side === "incoming" ? progress : 1 - progress;
}

function transitionVisualStyle(
  transition: PreviewTransition,
  progress: number,
  side: "incoming" | "outgoing",
): CSSProperties {
  const sideProgress = transitionSideProgress(transition, progress, side);
  if (isSlideLeft(transition.kind)) {
    return {
      transform:
        side === "incoming"
          ? `translateX(${(1 - sideProgress) * 100}%)`
          : `translateX(${-sideProgress * 100}%)`,
    };
  }
  if (isSlideRight(transition.kind)) {
    return {
      transform:
        side === "incoming"
          ? `translateX(${-(1 - sideProgress) * 100}%)`
          : `translateX(${sideProgress * 100}%)`,
    };
  }
  if (isWipeLeft(transition.kind)) {
    const pct = side === "incoming" ? sideProgress * 100 : (1 - sideProgress) * 100;
    return { clipPath: `inset(0 ${100 - pct}% 0 0)` };
  }
  if (isWipeRight(transition.kind)) {
    const pct = side === "incoming" ? sideProgress * 100 : (1 - sideProgress) * 100;
    return { clipPath: `inset(0 0 0 ${100 - pct}%)` };
  }
  if (isZoomIn(transition.kind) && side === "incoming") {
    return { transform: `scale(${0.86 + sideProgress * 0.14})` };
  }
  if (isRadial(transition.kind)) {
    const radius = side === "incoming" ? sideProgress * 75 : (1 - sideProgress) * 75;
    return { clipPath: `circle(${radius}% at 50% 50%)` };
  }
  if (isPixelize(transition.kind)) {
    const blur = side === "incoming" ? (1 - sideProgress) * 8 : sideProgress * 8;
    return {
      filter: `blur(${blur}px) contrast(${1 + Math.max(0, blur - 2) * 0.08})`,
      imageRendering: blur > 1 ? "pixelated" : "auto",
    };
  }
  return {};
}

function transitionSideProgress(
  transition: PreviewTransition,
  progress: number,
  side: "incoming" | "outgoing",
): number {
  const inShare =
    transition.duration <= 0
      ? 0.5
      : Math.max(0, Math.min(1, transition.inOffset / transition.duration));
  if (side === "incoming") {
    return inShare <= 0 ? 1 : Math.max(0, Math.min(1, progress / inShare));
  }
  const outShare = Math.max(0.0001, 1 - inShare);
  return Math.max(0, Math.min(1, (progress - inShare) / outShare));
}

function isDissolveTransition(kind: string): boolean {
  return kind === "SMPTE_Dissolve" || kind === "montage.cross_dissolve" || kind === "fade";
}

function isFadeThroughBlack(kind: string): boolean {
  return (
    kind === "montage.fade_black" ||
    kind === "fadeblack" ||
    kind === "montage.fade_in" ||
    kind === "montage.fade_out"
  );
}

function isFlashWhite(kind: string): boolean {
  return kind === "montage.flash_white" || kind === "fadewhite";
}

function isSlideLeft(kind: string): boolean {
  return (
    kind === "montage.slide_left" ||
    kind === "montage.smooth_push_left" ||
    kind === "slideleft" ||
    kind === "smoothleft"
  );
}

function isSlideRight(kind: string): boolean {
  return kind === "montage.slide_right" || kind === "slideright" || kind === "smoothright";
}

function isWipeLeft(kind: string): boolean {
  return kind === "montage.wipe_left" || kind === "wipeleft";
}

function isWipeRight(kind: string): boolean {
  return kind === "montage.wipe_right" || kind === "wiperight";
}

function isZoomIn(kind: string): boolean {
  return kind === "montage.zoom_in" || kind === "zoomin";
}

function isRadial(kind: string): boolean {
  return kind === "montage.radial" || kind === "radial";
}

function isPixelize(kind: string): boolean {
  return kind === "montage.pixelize" || kind === "pixelize";
}
