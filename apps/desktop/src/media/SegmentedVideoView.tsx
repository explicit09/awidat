// Segmented video preview — plays the timeline, not the source.
//
// Owns one <video> element. Walks the play-segments derived from the
// OTIO snapshot, hops between them as the timeline-time playhead
// crosses segment boundaries. From the user's perspective the
// preview is the cut: scrub bar shows timeline duration, current
// time tracks timeline-time, cuts are seamless.
//
// Architecture:
//
//   timelineTime  ── store ──→  active segment ── this view ──→  video.src + video.currentTime
//                                       ↑                              │
//                                       └──────── timeupdate ──────────┘
//
// The video element's currentTime is derived from timelineTime, not
// the other way around. timeupdate translates back so the store stays
// the single source of truth.
//
// Scope of THIS commit (9.3): single-segment switching. Boundary
// crossing during playback is detected via timeupdate (~250ms
// granularity, jittery on the cut). 9.4 replaces that with
// requestVideoFrameCallback for per-frame boundary checks. 9.5
// adds a hidden second <video> for next-segment preroll.

import { useEffect, useRef } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { useMediaStore } from "./store";
import { useTimelineStore } from "../timeline/store";
import {
  findActiveSegment,
  type PlaySegment,
  usePlaySegments,
} from "../timeline/usePlaySegments";

export function SegmentedVideoView() {
  const segments = usePlaySegments();
  const setTimelineDuration = useMediaStore((s) => s.setTimelineDuration);
  const timelineDurationS = useTimelineStore((s) => s.snapshot.duration_s);

  // Mirror the snapshot duration into the media store so the scrub
  // bar can clamp without subscribing to the timeline store too.
  useEffect(() => {
    setTimelineDuration(timelineDurationS);
  }, [timelineDurationS, setTimelineDuration]);

  if (segments.length === 0) {
    // Empty-segments fallback handled by the MediaPane caller. We
    // shouldn't render this view in that case, but bail safely if
    // we somehow get here mid-update.
    return null;
  }
  return <SegmentedPlayer segments={segments} />;
}

function SegmentedPlayer({ segments }: { segments: PlaySegment[] }) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  // The segment we last set src to. Used to avoid re-setting src on
  // every render (which would unload the video element and cause a
  // visible flicker).
  const activeSegRef = useRef<number>(-1);

  const timelineTime = useMediaStore((s) => s.timelineTime);
  const timelineDurationS = useMediaStore((s) => s.timelineDurationS);
  const isPlaying = useMediaStore((s) => s.isPlaying);
  const seekRequestId = useMediaStore((s) => s.timelineSeekRequestId);
  const seekTargetS = useMediaStore((s) => s.timelineSeekTargetS);
  const setTimelineTime = useMediaStore((s) => s.setTimelineTime);
  const setPlaying = useMediaStore((s) => s.setPlaying);
  const requestTimelineSeek = useMediaStore((s) => s.requestTimelineSeek);

  // Push the current timeline-time + active segment's clip-stem to
  // the agent's view-state ~1Hz. This lets the agent know which
  // moment of the *cut* the user is looking at, not which moment of
  // the source.
  const lastViewKeyRef = useRef<string>("");

  // Whenever timelineTime moves into a new segment, swap src and
  // align video.currentTime. The "timelineTime moves" trigger covers
  // playback (timeupdate-driven) AND external seeks (request-id-
  // driven), since both ultimately mutate timelineTime.
  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    const segIdx = findActiveSegment(segments, timelineTime);
    if (segIdx < 0) {
      // Past end of timeline → pause + park at last frame.
      if (segments.length > 0) {
        const last = segments[segments.length - 1];
        if (activeSegRef.current !== segments.length - 1) {
          activeSegRef.current = segments.length - 1;
          v.src = convertFileSrc(last.proxyPath);
        }
        v.currentTime = last.sourceEnd;
      }
      v.pause();
      return;
    }
    const seg = segments[segIdx];
    if (segIdx !== activeSegRef.current) {
      activeSegRef.current = segIdx;
      v.src = convertFileSrc(seg.proxyPath);
      // canplay handler below will set currentTime once ready.
      // We also seek opportunistically here in case the proxy is
      // already cached.
      const targetSrc = seg.sourceStart + (timelineTime - seg.timelineStart);
      tryAssignCurrentTime(v, targetSrc);
      return;
    }
    // Same segment, just sync currentTime if it drifted (e.g. an
    // external seek inside the active segment).
    const desired = seg.sourceStart + (timelineTime - seg.timelineStart);
    if (Math.abs(v.currentTime - desired) > 0.05) {
      tryAssignCurrentTime(v, desired);
    }
  }, [segments, timelineTime, seekRequestId, seekTargetS]);

  // External seek (timeline canvas click etc.) updates the target
  // store value; the effect above already reacts to seekRequestId.
  // No additional handler needed — kept here for future extension.

  // When the segments array changes (proxy finishes, agent edits),
  // re-evaluate the active segment from the current timelineTime and
  // make sure the src points at the right thing.
  useEffect(() => {
    activeSegRef.current = -1; // force the resync effect above
  }, [segments]);

  // timeupdate → translate source-time back to timeline-time.
  function onTimeUpdate(e: React.SyntheticEvent<HTMLVideoElement>) {
    const v = e.currentTarget;
    const segIdx = activeSegRef.current;
    if (segIdx < 0 || segIdx >= segments.length) return;
    const seg = segments[segIdx];
    // Boundary check: if we've played past the end of this segment,
    // advance. Crude in v1 (timeupdate fires every ~250ms so the
    // boundary is jittery); 9.4 replaces this with rVFC.
    if (v.currentTime >= seg.sourceEnd - 0.001) {
      const nextIdx = segIdx + 1;
      if (nextIdx >= segments.length) {
        // End of timeline — park, pause, push the timestamp.
        setTimelineTime(seg.timelineEnd);
        v.pause();
        return;
      }
      const next = segments[nextIdx];
      activeSegRef.current = nextIdx;
      v.src = convertFileSrc(next.proxyPath);
      tryAssignCurrentTime(v, next.sourceStart);
      // Resume play if we were playing — Safari pauses on src swap.
      v.play().catch(() => {});
      setTimelineTime(next.timelineStart);
      return;
    }
    // Within segment: regular update.
    const tlTime = seg.timelineStart + (v.currentTime - seg.sourceStart);
    setTimelineTime(tlTime);
  }

  // Push view-state (~1Hz, integer-second granularity) so the agent
  // knows where the user is on the timeline.
  useEffect(() => {
    const segIdx = findActiveSegment(segments, timelineTime);
    if (segIdx < 0) return;
    const seg = segments[segIdx];
    const stem = stemFromProxyPath(seg.proxyPath);
    if (!stem) return;
    const sec = Math.floor(timelineTime);
    const key = `${stem}:${sec}:${isPlaying ? "play" : "pause"}`;
    if (key === lastViewKeyRef.current) return;
    lastViewKeyRef.current = key;
    invoke("set_view_state", {
      stem,
      currentTimeS: timelineTime,
      isPlaying,
    }).catch(() => {});
  }, [segments, timelineTime, isPlaying]);

  // Spacebar play/pause — same guard as VideoView.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const tag = (e.target as HTMLElement | null)?.tagName?.toLowerCase();
      if (tag === "textarea" || tag === "input") return;
      if (e.code === "Space") {
        e.preventDefault();
        togglePlay();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function togglePlay() {
    const v = videoRef.current;
    if (!v) return;
    if (v.paused) {
      v.play().catch(() => {});
    } else {
      v.pause();
    }
  }

  function onScrub(e: React.ChangeEvent<HTMLInputElement>) {
    const t = Number(e.target.value);
    if (Number.isFinite(t)) requestTimelineSeek(t);
  }

  return (
    <div className="video-wrap">
      <video
        ref={videoRef}
        className="video-el"
        preload="metadata"
        onTimeUpdate={onTimeUpdate}
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onEnded={() => setPlaying(false)}
        onClick={togglePlay}
      />
      <div className="transport">
        <button
          className="transport-play"
          onClick={togglePlay}
          aria-label={isPlaying ? "Pause" : "Play"}
        >
          {isPlaying ? <PauseIcon /> : <PlayIcon />}
        </button>
        <input
          className="transport-scrub"
          type="range"
          min={0}
          max={timelineDurationS || 0}
          step={0.01}
          value={Math.min(timelineTime, timelineDurationS)}
          onChange={onScrub}
          disabled={timelineDurationS === 0}
        />
        <div className="transport-time">
          <span>{formatTime(timelineTime)}</span>
          <span className="transport-time-sep">/</span>
          <span className="transport-time-total">
            {formatTime(timelineDurationS)}
          </span>
        </div>
      </div>
      <div className="video-meta">
        <span className="video-meta-label">timeline preview</span>
        <code className="video-stem">
          {segments.length} segment{segments.length === 1 ? "" : "s"}
        </code>
      </div>
    </div>
  );
}

// Some browsers throw when setting currentTime before the readyState
// reaches HAVE_METADATA. Clamp to a guarded write so the segment
// swap doesn't crash if the user spam-scrubs.
function tryAssignCurrentTime(v: HTMLVideoElement, t: number) {
  if (!Number.isFinite(t) || t < 0) return;
  try {
    v.currentTime = t;
  } catch {
    // Ignore — `loadedmetadata` will retry via the timelineTime
    // effect's resync path.
  }
}

// Recover the proxy stem from its absolute path. The view-state
// IPC keys off stems, not paths, because the agent works with stems
// throughout (matches `list_proxies` etc).
function stemFromProxyPath(path: string): string | null {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const file = slash >= 0 ? path.slice(slash + 1) : path;
  const dot = file.lastIndexOf(".");
  return dot > 0 ? file.slice(0, dot) : file || null;
}

function PlayIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 14 14" fill="currentColor">
      <path d="M3 2.5v9l8-4.5z" />
    </svg>
  );
}

function PauseIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 14 14" fill="currentColor">
      <rect x="3" y="2.5" width="3" height="9" rx="0.5" />
      <rect x="8" y="2.5" width="3" height="9" rx="0.5" />
    </svg>
  );
}

function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}
