import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { cachedMediaStreamUrl, mediaStreamUrl } from "../mediaStreamUrl";
import { videoOverlayStyle as buildVideoOverlayStyle } from "../videoOverlayStyle";
import { tryAssignCurrentTime } from "../videoElement";
import {
  safeSegmentSpeed,
  sourceTimeForTimelineTime,
  type VideoOverlaySegment,
} from "../../timeline/usePlaySegments";

function TimelineVideoOverlays({
  overlays,
  timelineTime,
  isPlaying,
}: {
  overlays: VideoOverlaySegment[];
  timelineTime: number;
  isPlaying: boolean;
}) {
  if (overlays.length === 0) return null;
  return (
    <div className="timeline-video-overlay-layer" aria-hidden="true">
      {overlays.map((overlay) => (
        <TimelineVideoOverlay
          key={`${overlay.proxyPath}:${overlay.timelineStart}:${overlay.zIndex}`}
          overlay={overlay}
          timelineTime={timelineTime}
          isPlaying={isPlaying}
        />
      ))}
    </div>
  );
}

function TimelineVideoOverlay({
  overlay,
  timelineTime,
  isPlaying,
}: {
  overlay: VideoOverlaySegment;
  timelineTime: number;
  isPlaying: boolean;
}) {
  const ref = useRef<HTMLVideoElement | null>(null);
  const lastSyncKeyRef = useRef<string>("");
  const [previewHeightPx, setPreviewHeightPx] = useState<number | null>(null);
  const [src, setSrc] = useState<string | null>(() =>
    cachedMediaStreamUrl(overlay.proxyPath) ?? null,
  );

  useLayoutEffect(() => {
    const element = ref.current;
    const layer = element?.parentElement;
    if (!layer) return;
    const update = () => setPreviewHeightPx(layer.getBoundingClientRect().height);
    update();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(update);
    observer.observe(layer);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const cached = cachedMediaStreamUrl(overlay.proxyPath);
    if (cached) {
      setSrc(cached);
      return;
    }
    let cancelled = false;
    mediaStreamUrl(overlay.proxyPath)
      .then((url) => {
        if (!cancelled) setSrc(url);
      })
      .catch((e) => {
        console.warn("overlay media stream url failed", e);
      });
    return () => {
      cancelled = true;
    };
  }, [overlay.proxyPath]);

  useEffect(() => {
    const v = ref.current;
    if (!v) return;
    const desired = sourceTimeForTimelineTime(overlay, timelineTime);
    const syncKey = `${overlay.proxyPath}:${overlay.timelineStart}:${overlay.zIndex}`;
    const force = syncKey !== lastSyncKeyRef.current || !isPlaying || v.paused;
    lastSyncKeyRef.current = syncKey;
    const drift = Math.abs((v.currentTime || 0) - desired);
    if (Number.isFinite(desired) && drift > (force ? 0.02 : 0.5)) {
      tryAssignCurrentTime(v, desired);
    }
    const speed = safeSegmentSpeed(overlay.speed);
    if (Math.abs(v.playbackRate - speed) > 0.001) v.playbackRate = speed;
    v.muted = true;
    if (isPlaying && v.paused) {
      v.play().catch(() => {});
    } else if (!isPlaying && !v.paused) {
      v.pause();
    }
  }, [overlay, timelineTime, isPlaying]);

  if (!src) return null;
  return (
    <video
      ref={ref}
      className="timeline-video-overlay"
      src={src}
      muted
      playsInline
      preload="auto"
      style={videoOverlayStyle({ ...overlay, previewHeightPx }, timelineTime)}
    />
  );
}

function videoOverlayStyle(
  overlay: VideoOverlaySegment & { previewHeightPx?: number | null },
  timelineTime: number,
): React.CSSProperties {
  return buildVideoOverlayStyle(overlay, timelineTime);
}

export { TimelineVideoOverlays, TimelineVideoOverlay, videoOverlayStyle };
