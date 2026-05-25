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
// fetch anything: `playable_path` is already on every clip, resolved
// by the backend in `flatten_timeline_public`. Memoized on snapshot
// identity so the SegmentedVideoView doesn't re-derive on every
// render.
//
// The base program comes from the first non-title video track; upper
// video tracks are composited as overlays for B-roll/PiP. Until the
// preview owns a separate audio mixer, linked audio tracks are
// visual/editing state only; the proxy video remains the audible
// preview source.

import { useMemo } from "react";
import type { TimelineParameterAnimation } from "../protocol";
import { useTimelineStore, type TimelineSnapshot } from "./store";

export type PlaySegment = {
  /** Absolute path to the proxy mp4 — pass to convertFileSrc(). */
  proxyPath: string;
  /** Proxy stem (filename without extension). Same id used by
   *  `useMediaStore.proxies[].stem` and `read_transcript(stem)`. */
  proxyStem: string;
  /** Project-relative path of the *source* asset (e.g. `raw/foo.MOV`).
   *  Distinct from `proxyPath` — agent tools look this up under the
   *  project root, so view-state hints must use this id, not the
   *  proxy stem (which lives under `.awidat/proxies/`). */
  assetId: string | null;
  /** Stem of the source asset (filename without extension). The
   *  agent's view-context line uses this so e.g. `view_frame` can
   *  resolve it against `raw/<stem>.<ext>`. */
  sourceStem: string | null;
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
  animations: TimelineParameterAnimation[];
};

export type PreviewTransition = {
  /** OTIO effect_name, e.g. `SMPTE_Dissolve` or a raw FFmpeg xfade name. */
  kind: string;
  /**
   * Stable awidat transition id, e.g. `awidat.match_dissolve`. `null`
   * when the transition has no `metadata.awidat_transition`. Used by
   * the preview to decide whether to route through the GPU renderer
   * (when the transition's composition wants a shader CSS can't fake)
   * or fall back to the CSS-based overlay.
   */
  transitionId: string | null;
  cutTime: number;
  inOffset: number;
  outOffset: number;
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

export const AUTO_ADVANCE_GAP_LIMIT_S = 0.25;

/**
 * Walk the snapshot's first video track and produce the playable
 * segments. Empty array when:
 *   - no project loaded
 *   - timeline has no video clips yet (pre-import / pre-auto-insert)
 *   - every clip's playable_path is null or missing
 *
 * The empty case lets the preview pane show either "Generating
 * preview..." for clips awaiting proxies or the no-clips empty state.
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
        const playable = item.playable_path;
        if (
          playable === null ||
          playable === undefined ||
          (item.playable_kind !== "proxy" && item.playable_kind !== "source") ||
          item.duration_s <= 0
        ) {
          // Asset is gone or clip has zero length. Tier 5 surfaces
          // the missing-media state separately.
          continue;
        }
        const sourceStart = item.source_start_s ?? 0;
        const overlay = item.video_overlay;
        const isPip = overlay?.mode === "pip";
        segments.push({
          proxyPath: playable,
          proxyStem: stemFromProxyPath(playable),
          assetId: item.asset_id ?? null,
          sourceStem: stemFromAssetId(item.asset_id),
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
          animations: item.animations ?? [],
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

/** Extract the stem (filename minus extension) from a project-relative
 *  asset id like `raw/foo.MOV` → `foo`. Returns null when the input
 *  is null/empty. */
function stemFromAssetId(assetId: string | null | undefined): string | null {
  if (!assetId) return null;
  const slash = Math.max(assetId.lastIndexOf("/"), assetId.lastIndexOf("\\"));
  const file = slash >= 0 ? assetId.slice(slash + 1) : assetId;
  const dot = file.lastIndexOf(".");
  return dot > 0 ? file.slice(0, dot) : file || null;
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

export function derivePreviewPlan(snapshot: TimelineSnapshot): PreviewPlan {
  const videoTrack = snapshot.tracks.find(
    (t) => t.kind === "video" && t.role !== "titles",
  );
  if (!videoTrack) return { segments: [], transitions: [], duration: 0 };
  const segments: PlaySegment[] = [];
  const transitions: PreviewTransition[] = [];
  let pendingTransition: {
    kind: string;
    transitionId: string | null;
    duration: number;
    inOffset: number;
    outOffset: number;
  } | null = null;

  for (const item of videoTrack.items) {
    if (item.kind === "gap") {
      pendingTransition = null;
      continue;
    }
    if (item.kind === "transition") {
      pendingTransition = {
        kind: item.effect_name,
        transitionId: item.transition_id ?? null,
        duration: Math.max(0, item.duration_s),
        inOffset: Math.max(0, item.in_offset_s),
        outOffset: Math.max(0, item.out_offset_s),
      };
      continue;
    }
    if (item.kind !== "clip") continue;
    const playable = item.playable_path;
    if (
      playable === null ||
      playable === undefined ||
      (item.playable_kind !== "proxy" && item.playable_kind !== "source") ||
      item.duration_s <= 0
    ) {
      pendingTransition = null;
      continue;
    }

    const originalSourceStart = item.source_start_s ?? 0;
    const segment: PlaySegment = {
      proxyPath: playable,
      proxyStem: stemFromProxyPath(playable),
      assetId: item.asset_id ?? null,
      sourceStem: stemFromAssetId(item.asset_id),
      sourceStart: originalSourceStart,
      sourceEnd: originalSourceStart + item.duration_s,
      timelineStart: item.track_start_s,
      timelineEnd: item.track_start_s + item.duration_s,
      volume: item.volume ?? 1,
      speed: item.speed ?? 1,
      clipIndex: item.index,
    };

    const previous = segments[segments.length - 1];
    if (pendingTransition && previous && pendingTransition.duration > 0) {
      const duration = Math.min(
        pendingTransition.duration,
        Math.max(0, previous.timelineEnd - previous.timelineStart),
        Math.max(0, segment.timelineEnd - segment.timelineStart),
      );
      if (duration > 0) {
        const inOffset = Math.min(pendingTransition.inOffset, duration);
        const outOffset = Math.min(
          pendingTransition.outOffset,
          Math.max(0, duration - inOffset),
        );
        const cutTime = segment.timelineStart;
        transitions.push({
          kind: pendingTransition.kind,
          transitionId: pendingTransition.transitionId,
          cutTime,
          inOffset,
          outOffset,
          timelineStart: cutTime - inOffset,
          timelineEnd: cutTime + outOffset,
          duration,
          from: previous,
          to: segment,
        });
      }
    }

    segments.push(segment);
    pendingTransition = null;
  }

  return {
    segments,
    transitions,
    duration: snapshot.duration_s,
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

/**
 * Return the first segment that starts after `timelineTime`.
 * Used by preview playback to recover from seeks into timeline gaps:
 * while the render path can output black/silence, the live proxy
 * player can only play real media segments.
 */
export function findNextSegmentAfter(
  segments: PlaySegment[],
  timelineTime: number,
): number {
  for (let index = 0; index < segments.length; index += 1) {
    if (timelineTime < segments[index].timelineStart) {
      return index;
    }
  }
  return -1;
}

export function shouldAutoAdvanceTimelineGap(
  timelineTime: number,
  nextTimelineStart: number,
): boolean {
  const gapS = nextTimelineStart - timelineTime;
  return gapS >= 0 && gapS <= AUTO_ADVANCE_GAP_LIMIT_S;
}
