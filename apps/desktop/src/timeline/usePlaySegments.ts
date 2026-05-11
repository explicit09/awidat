// Derive playable segments from the OTIO timeline snapshot.
//
// A "play segment" is one slice of one proxy mp4 to play during a
// specific timeline-time window. The SegmentedVideoView walks this
// array as the playhead advances:
//
//   timelineTime = 0..   → segment 0: proxy-A at source[5..30]
//   timelineTime = 25..  → segment 1: proxy-A at source[40..50]
//   timelineTime = 35..  → segment 2: proxy-B at source[0..12]
//   ...
//
// A pro NLE (Resolve, Premiere, Final Cut) does the same thing —
// the preview is the timeline output, not the source media. Cuts
// are stitched by the player at boundary moments, no re-render
// needed because the proxies are all-keyframe mp4s and the seek
// to the next segment is sub-frame.
//
// This hook is a pure derivation from the snapshot. We do NOT
// fetch anything: `proxy_path` is already on every clip, resolved
// by the backend in `flatten_timeline_public`. Memoized on snapshot
// identity so the SegmentedVideoView doesn't re-derive on every
// render.
//
// v1 scope: video tracks only, single-track flat (V1). Multi-video
// (V2 overlay) lands when we tackle B-roll preview. When explicit
// audio tracks exist, preview mutes embedded video audio so it does
// not double with the first-class mix used by export.

import { useMemo } from "react";
import { useTimelineStore, type TimelineSnapshot } from "./store";

export type PlaySegment = {
  /** Absolute path to the proxy mp4 — pass to convertFileSrc(). */
  proxyPath: string;
  /** Proxy stem (filename without extension). Same id used by
   *  `useMediaStore.proxies[].stem` and `read_transcript(stem)`. */
  proxyStem: string;
  /** Start offset into the proxy media, in seconds. */
  sourceStart: number;
  /** End offset into the proxy media, in seconds. */
  sourceEnd: number;
  /** Where this segment begins on the master timeline, in seconds. */
  timelineStart: number;
  /** Where this segment ends on the master timeline, in seconds. */
  timelineEnd: number;
  /** Effective clip volume from the timeline, clamped by the player. */
  volume: number;
  /** Effective clip speed from the timeline, clamped by the player. */
  speed: number;
  /** Clip index inside its track. Useful for diagnostics + diff hints. */
  clipIndex: number;
};

export type VideoOverlaySegment = PlaySegment & {
  mode: "full_frame" | "pip";
  corner: "top_left" | "top_right" | "bottom_left" | "bottom_right";
  scale: number;
  marginPct: number;
  zIndex: number;
};

export type PreviewTransition = {
  kind: string;
  timelineStart: number;
  timelineEnd: number;
  duration: number;
  from: PlaySegment;
  to: PlaySegment;
};

type PreviewPlan = {
  segments: PlaySegment[];
  transitions: PreviewTransition[];
  duration: number;
};

/**
 * Walk the snapshot's first video track and produce the playable
 * segments. Empty array when:
 *   - no project loaded
 *   - timeline has no video clips yet (pre-import / pre-auto-insert)
 *   - every clip's proxy is still transcoding (proxy_path === null)
 *
 * The empty case signals to the MediaPane that it should fall back
 * to the source-asset preview (current behavior when there's nothing
 * to stitch).
 */
export function usePlaySegments(): PlaySegment[] {
  const snapshot = useTimelineStore((s) => s.snapshot);
  return useMemo(() => derivePreviewPlan(snapshot).segments, [snapshot]);
}

export function usePreviewTransitions(): PreviewTransition[] {
  const snapshot = useTimelineStore((s) => s.snapshot);
  return useMemo(() => derivePreviewPlan(snapshot).transitions, [snapshot]);
}

export function usePreviewDuration(): number {
  const snapshot = useTimelineStore((s) => s.snapshot);
  return useMemo(() => derivePreviewPlan(snapshot).duration, [snapshot]);
}

/**
 * Derive upper video-track overlays. The first non-title video track
 * is the base program; later non-title video tracks composite above it.
 */
export function useVideoOverlaySegments(): VideoOverlaySegment[] {
  const snapshot = useTimelineStore((s) => s.snapshot);
  return useMemo(() => {
    const videoTracks = snapshot.tracks.filter(
      (t) => t.kind === "video" && t.role !== "titles",
    );
    const overlayTracks = videoTracks.slice(1);
    const segments: VideoOverlaySegment[] = [];
    overlayTracks.forEach((track, trackOffset) => {
      for (const item of track.items) {
        if (item.kind !== "clip") continue;
        if (item.proxy_path === null || item.duration_s <= 0) continue;
        const sourceStart = item.source_start_s ?? 0;
        const overlay = item.video_overlay;
        const isPip = overlay?.mode === "pip";
        segments.push({
          proxyPath: item.proxy_path,
          proxyStem: stemFromProxyPath(item.proxy_path),
          sourceStart,
          sourceEnd: sourceStart + item.duration_s,
          timelineStart: item.track_start_s,
          timelineEnd: item.track_start_s + item.duration_s,
          volume: 0,
          speed: item.speed ?? 1,
          clipIndex: item.index,
          mode: isPip ? "pip" : "full_frame",
          corner: normalizeCorner(overlay?.corner),
          scale: clampNumber(overlay?.scale, 0.28, 0.1, 0.6),
          marginPct: clampNumber(overlay?.margin_pct, 0.035, 0, 0.15),
          zIndex: trackOffset,
        });
      }
    });
    segments.sort((a, b) => a.timelineStart - b.timelineStart || a.zIndex - b.zIndex);
    return segments;
  }, [snapshot]);
}

/** Extract the stem (filename minus extension) from a proxy path. */
function stemFromProxyPath(path: string): string {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const file = slash >= 0 ? path.slice(slash + 1) : path;
  const dot = file.lastIndexOf(".");
  return dot > 0 ? file.slice(0, dot) : file;
}

function normalizeCorner(
  corner: string | null | undefined,
): VideoOverlaySegment["corner"] {
  if (
    corner === "top_left" ||
    corner === "top_right" ||
    corner === "bottom_left" ||
    corner === "bottom_right"
  ) {
    return corner;
  }
  return "bottom_right";
}

function clampNumber(
  value: number | null | undefined,
  fallback: number,
  min: number,
  max: number,
): number {
  if (!Number.isFinite(value)) return fallback;
  return Math.max(min, Math.min(max, value as number));
}

function derivePreviewPlan(snapshot: TimelineSnapshot): PreviewPlan {
  const videoTrack = snapshot.tracks.find(
    (t) => t.kind === "video" && t.role !== "titles",
  );
  if (!videoTrack) return { segments: [], transitions: [], duration: 0 };
  const hasExplicitAudio = snapshot.tracks.some((t) => t.kind === "audio");

  const segments: PlaySegment[] = [];
  const transitions: PreviewTransition[] = [];
  let outputCursor = 0;
  let pendingTransition: { kind: string; duration: number } | null = null;

  for (const item of videoTrack.items) {
    if (item.kind === "gap") {
      outputCursor += item.duration_s;
      pendingTransition = null;
      continue;
    }
    if (item.kind === "transition") {
      pendingTransition = {
        kind: item.effect_name,
        duration: Math.max(0, item.duration_s),
      };
      continue;
    }
    if (item.kind !== "clip") continue;
    if (item.proxy_path === null || item.duration_s <= 0) {
      pendingTransition = null;
      continue;
    }

    const originalSourceStart = item.source_start_s ?? 0;
    const incomingTransition = pendingTransition;
    const incomingDuration = incomingTransition
      ? Math.min(incomingTransition.duration, item.duration_s)
      : 0;
    const playableDuration = Math.max(0, item.duration_s - incomingDuration);
    const sourceStart = originalSourceStart + incomingDuration;
    const segment: PlaySegment = {
      proxyPath: item.proxy_path,
      proxyStem: stemFromProxyPath(item.proxy_path),
      sourceStart,
      sourceEnd: originalSourceStart + item.duration_s,
      timelineStart: outputCursor,
      timelineEnd: outputCursor + playableDuration,
      volume: hasExplicitAudio ? 0 : item.volume ?? 1,
      speed: item.speed ?? 1,
      clipIndex: item.index,
    };

    const previous = segments[segments.length - 1];
    if (incomingTransition && previous && incomingDuration > 0) {
      const duration = Math.min(
        incomingDuration,
        Math.max(0, previous.timelineEnd - previous.timelineStart),
      );
      if (duration > 0) {
        transitions.push({
          kind: incomingTransition.kind,
          timelineStart: previous.timelineEnd - duration,
          timelineEnd: previous.timelineEnd,
          duration,
          from: previous,
          to: {
            ...segment,
            sourceStart: originalSourceStart,
            sourceEnd: originalSourceStart + duration,
            timelineStart: previous.timelineEnd - duration,
            timelineEnd: previous.timelineEnd,
          },
        });
      }
    }

    if (playableDuration > 0) {
      segments.push(segment);
      outputCursor = segment.timelineEnd;
    }
    pendingTransition = null;
  }

  return {
    segments,
    transitions,
    duration: Math.max(
      outputCursor,
      transitions.length > 0 ? transitions[transitions.length - 1].timelineEnd : 0,
    ),
  };
}

/**
 * Map a `(stem, source-time)` pair to its timeline-time, if the
 * stem appears as a segment whose source range covers the time.
 * Used by the transcript pane to seek the timeline preview to the
 * spot the user clicked. Returns `null` when the moment isn't
 * currently on the timeline (the clip was trimmed out).
 */
export function timelineTimeForSource(
  segments: PlaySegment[],
  stem: string,
  sourceTime: number,
): number | null {
  for (const seg of segments) {
    if (seg.proxyStem !== stem) continue;
    if (sourceTime < seg.sourceStart || sourceTime > seg.sourceEnd) continue;
    return seg.timelineStart + (sourceTime - seg.sourceStart);
  }
  return null;
}

/**
 * Best-effort variant for transcript clicks outside the current cut.
 * When the exact source time is trimmed out, jump to the closest
 * timeline occurrence for the same asset instead of falling back to
 * source-preview seeking while the timeline monitor is active.
 */
export function nearestTimelineTimeForSource(
  segments: PlaySegment[],
  stem: string,
  sourceTime: number,
): number | null {
  let best: { distance: number; timelineTime: number } | null = null;
  for (const seg of segments) {
    if (seg.proxyStem !== stem) continue;
    const clampedSource = Math.max(
      seg.sourceStart,
      Math.min(sourceTime, seg.sourceEnd),
    );
    const distance = Math.abs(sourceTime - clampedSource);
    const timelineTime = seg.timelineStart + (clampedSource - seg.sourceStart);
    if (!best || distance < best.distance) {
      best = { distance, timelineTime };
    }
  }
  return best?.timelineTime ?? null;
}

/**
 * Binary-search for the segment that owns `timelineTime`. Returns
 * the index, or -1 when the time falls in a gap or after the last
 * segment. The SegmentedVideoView uses -1 to render a black frame /
 * "end of timeline" state.
 *
 * Half-open ranges: `[timelineStart, timelineEnd)`. A time exactly
 * on a boundary belongs to the segment to the right.
 */
export function findActiveSegment(
  segments: PlaySegment[],
  timelineTime: number,
): number {
  if (segments.length === 0) return -1;
  if (timelineTime < segments[0].timelineStart) return -1;
  if (timelineTime >= segments[segments.length - 1].timelineEnd) return -1;

  let lo = 0;
  let hi = segments.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >>> 1;
    const seg = segments[mid];
    if (timelineTime < seg.timelineStart) {
      hi = mid - 1;
    } else if (timelineTime >= seg.timelineEnd) {
      lo = mid + 1;
    } else {
      return mid;
    }
  }
  return -1; // timeline-time falls in a gap between segments
}
