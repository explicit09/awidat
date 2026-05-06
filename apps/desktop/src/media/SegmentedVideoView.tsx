// Segmented video preview — plays the timeline, not the source.
//
// Walks the play-segments derived from the OTIO snapshot, hopping
// between them as the timeline-time playhead crosses segment
// boundaries. From the user's perspective the preview is the cut:
// scrub bar shows timeline duration, current time tracks timeline-
// time, cuts are seamless.
//
// **Double-buffered playback (Step 9.5):** two <video> elements
// stacked in the same spot. One is "active" (visible, playing); the
// other is "preroll" (hidden, src already pointing at the next
// segment with currentTime parked at next.sourceStart). At a
// boundary cross we just flip which is on top — no src reassignment
// on the visible element, so no decoder warmup, no flash.
//
//   active   ──── playing ────→ boundary ─┐
//   preroll  ─── ready, paused ──────────→├─→ swap roles
//                                          ↓
//   active'  ─── plays from sourceStart ──→ next boundary
//   preroll' ─── load after-next ────────→ ready
//
// Same-asset back-to-back segments (the awidat common case) get
// minor benefit since the proxy is already cached; cross-asset
// crossings get the full benefit (no flash).
//
// Architecture:
//
//   timelineTime  ── store ──→  active segment ── this view ──→  active video
//                                       ↑                             │
//                                       └──── rVFC tick ──────────────┘
//
// Boundary detection is requestVideoFrameCallback (Step 9.4) when
// available; timeupdate fallback otherwise.

import { useEffect, useMemo, useRef, useState } from "react";
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

// One slot in the double-buffer.
type Slot = {
  ref: React.MutableRefObject<HTMLVideoElement | null>;
  /** Segment index this slot is loaded with, or -1 when empty. */
  segIdx: number;
};

function SegmentedPlayer({ segments }: { segments: PlaySegment[] }) {
  const refA = useRef<HTMLVideoElement | null>(null);
  const refB = useRef<HTMLVideoElement | null>(null);
  // The currently-visible slot. The other slot is the preroll.
  const [activeKey, setActiveKey] = useState<"a" | "b">("a");
  // What each slot has loaded. Refs (not state) so the rVFC tick
  // can advance them without triggering renders mid-frame.
  const slotsRef = useRef<{ a: Slot; b: Slot }>({
    a: { ref: refA, segIdx: -1 },
    b: { ref: refB, segIdx: -1 },
  });

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

  // Latest segments captured into a ref so the rVFC tick can read
  // them without forcing the rAF-style callback to re-register on
  // every render.
  const segmentsRef = useRef<PlaySegment[]>(segments);
  useEffect(() => {
    segmentsRef.current = segments;
  }, [segments]);

  // activeKey captured into a ref for the same reason.
  const activeKeyRef = useRef(activeKey);
  useEffect(() => {
    activeKeyRef.current = activeKey;
  }, [activeKey]);

  // Whenever timelineTime moves into a new segment, make sure the
  // active slot is loaded with that segment's proxy and aligned.
  // Triggers cover playback, external seeks (request-id), and
  // segment-array changes (proxy-finished, agent-edit).
  useEffect(() => {
    const segIdx = findActiveSegment(segments, timelineTime);
    const active = slotsRef.current[activeKeyRef.current];
    const v = active.ref.current;
    if (!v) return;
    if (segIdx < 0) {
      // Past end of timeline → pause + park at last frame.
      if (segments.length > 0) {
        const last = segments.length - 1;
        ensureSlotLoaded(active, segments[last]);
        active.segIdx = last;
        v.currentTime = segments[last].sourceEnd;
      }
      v.pause();
      return;
    }
    const seg = segments[segIdx];
    if (active.segIdx !== segIdx) {
      ensureSlotLoaded(active, seg);
      active.segIdx = segIdx;
    }
    const desired = seg.sourceStart + (timelineTime - seg.timelineStart);
    if (Math.abs(v.currentTime - desired) > 0.05) {
      tryAssignCurrentTime(v, desired);
    }
    // Preload the next segment into the inactive slot.
    primePreroll(segIdx);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [segments, timelineTime, seekRequestId, seekTargetS]);

  // Reset both slots when the segments array identity changes (e.g.
  // an apply_edl landed). Forces the resync effect above to re-attach
  // src on the new active segment.
  useEffect(() => {
    slotsRef.current.a.segIdx = -1;
    slotsRef.current.b.segIdx = -1;
  }, [segments]);

  // Set src + park at sourceStart for a slot. Idempotent — if the
  // slot already has the right src loaded, we skip the assignment
  // (re-setting src reloads the decoder, which is exactly what
  // double-buffering is meant to avoid).
  function ensureSlotLoaded(slot: Slot, seg: PlaySegment) {
    const v = slot.ref.current;
    if (!v) return;
    const desiredSrc = convertFileSrc(seg.proxyPath);
    if (v.src !== desiredSrc) {
      v.src = desiredSrc;
    }
    tryAssignCurrentTime(v, seg.sourceStart);
  }

  // Prime the inactive slot with the segment AFTER `currentSegIdx`
  // so it's ready when we cross the boundary. Pauses the preroll
  // slot — only the active slot plays.
  function primePreroll(currentSegIdx: number) {
    const segs = segmentsRef.current;
    const nextIdx = currentSegIdx + 1;
    const inactiveKey: "a" | "b" = activeKeyRef.current === "a" ? "b" : "a";
    const inactive = slotsRef.current[inactiveKey];
    const v = inactive.ref.current;
    if (!v) return;
    if (nextIdx >= segs.length) {
      // Nothing to preroll — clear so we don't leave stale media
      // taking up decoder slots. Setting empty src is safe.
      if (inactive.segIdx !== -1) {
        v.removeAttribute("src");
        v.load();
        inactive.segIdx = -1;
      }
      return;
    }
    const next = segs[nextIdx];
    if (inactive.segIdx !== nextIdx) {
      ensureSlotLoaded(inactive, next);
      inactive.segIdx = nextIdx;
    }
    if (!v.paused) v.pause();
  }

  // Per-frame boundary check + time translation. Called from rVFC
  // (preferred) and from `timeupdate` as a fallback.
  function tickAtVideoTime(v: HTMLVideoElement, vTime: number) {
    const segs = segmentsRef.current;
    const activeSlot = slotsRef.current[activeKeyRef.current];
    const segIdx = activeSlot.segIdx;
    if (segIdx < 0 || segIdx >= segs.length) return;
    const seg = segs[segIdx];
    if (vTime >= seg.sourceEnd - 0.001) {
      const nextIdx = segIdx + 1;
      if (nextIdx >= segs.length) {
        // End of timeline.
        setTimelineTime(seg.timelineEnd);
        v.pause();
        return;
      }
      // Boundary cross: flip to the prerolled slot.
      const inactiveKey: "a" | "b" = activeKeyRef.current === "a" ? "b" : "a";
      const inactive = slotsRef.current[inactiveKey];
      const nextV = inactive.ref.current;
      const next = segs[nextIdx];
      // Defensive: if preroll didn't get a chance to load (rapid
      // segment churn, or the user scrubbed past the cut), force-
      // load the inactive slot now. Same flow as the single-buffer
      // case — visible flash, but at least it plays.
      if (inactive.segIdx !== nextIdx && nextV) {
        ensureSlotLoaded(inactive, next);
        inactive.segIdx = nextIdx;
      } else if (nextV) {
        // Preroll was correct; still seek to sourceStart in case
        // the user jumped back-and-forth and left it elsewhere.
        tryAssignCurrentTime(nextV, next.sourceStart);
      }
      // Pause the leaving slot, play the entering slot.
      v.pause();
      if (nextV) nextV.play().catch(() => {});
      // Flip the visible slot. activeKey state drives the CSS
      // class swap; activeKeyRef updates on the next render via
      // the sync useEffect above.
      setActiveKey(inactiveKey);
      activeKeyRef.current = inactiveKey;
      setTimelineTime(next.timelineStart);
      // Kick the now-inactive slot to start prerolling segment
      // after-next (if any).
      primePreroll(nextIdx);
      return;
    }
    // Within segment: smooth playhead translation.
    setTimelineTime(seg.timelineStart + (vTime - seg.sourceStart));
  }

  // Mount rVFC on the active slot. Re-mounts when activeKey flips
  // so we always observe the playing video.
  useEffect(() => {
    const v = slotsRef.current[activeKey].ref.current;
    if (!v) return;
    type RVFCMetadata = { mediaTime: number };
    type RVFCVideo = HTMLVideoElement & {
      requestVideoFrameCallback?: (
        cb: (now: number, metadata: RVFCMetadata) => void,
      ) => number;
      cancelVideoFrameCallback?: (id: number) => void;
    };
    const rvfcVideo = v as RVFCVideo;
    if (typeof rvfcVideo.requestVideoFrameCallback !== "function") {
      // Fallback: timeupdate handler runs unmodified.
      return;
    }
    let cancelled = false;
    let handle = 0;
    const tick = (_now: number, metadata: RVFCMetadata) => {
      if (cancelled) return;
      tickAtVideoTime(v, metadata.mediaTime);
      const next = rvfcVideo.requestVideoFrameCallback;
      if (next) handle = next(tick);
    };
    handle = rvfcVideo.requestVideoFrameCallback(tick);
    return () => {
      cancelled = true;
      rvfcVideo.cancelVideoFrameCallback?.(handle);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeKey]);

  // timeupdate fallback. Idempotent with rVFC — boundary advance
  // checks `activeSlot.segIdx` which only changes once per cross.
  function onTimeUpdate(e: React.SyntheticEvent<HTMLVideoElement>) {
    tickAtVideoTime(e.currentTarget, e.currentTarget.currentTime);
  }

  // Push view-state (~1Hz, integer-second granularity).
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
    const v = slotsRef.current[activeKeyRef.current].ref.current;
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

  // The hidden slot uses opacity 0 + pointer-events: none so it
  // doesn't intercept clicks on the visible video. Both elements
  // are positioned absolutely in the same box; the visible slot
  // has opacity 1 and z-index above. We avoid `display: none` and
  // `visibility: hidden` because both can pause buffering on some
  // browsers.
  const styleA = useMemo(
    () => slotStyle(activeKey === "a"),
    [activeKey],
  );
  const styleB = useMemo(
    () => slotStyle(activeKey === "b"),
    [activeKey],
  );

  return (
    <div className="video-wrap">
      <div className="video-stack">
        <video
          ref={refA}
          className="video-el"
          preload="auto"
          style={styleA}
          onTimeUpdate={activeKey === "a" ? onTimeUpdate : undefined}
          onPlay={activeKey === "a" ? () => setPlaying(true) : undefined}
          onPause={activeKey === "a" ? () => setPlaying(false) : undefined}
          onEnded={activeKey === "a" ? () => setPlaying(false) : undefined}
          onClick={activeKey === "a" ? togglePlay : undefined}
        />
        <video
          ref={refB}
          className="video-el"
          preload="auto"
          style={styleB}
          onTimeUpdate={activeKey === "b" ? onTimeUpdate : undefined}
          onPlay={activeKey === "b" ? () => setPlaying(true) : undefined}
          onPause={activeKey === "b" ? () => setPlaying(false) : undefined}
          onEnded={activeKey === "b" ? () => setPlaying(false) : undefined}
          onClick={activeKey === "b" ? togglePlay : undefined}
        />
      </div>
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

function slotStyle(visible: boolean): React.CSSProperties {
  return {
    position: "absolute",
    inset: 0,
    width: "100%",
    height: "100%",
    opacity: visible ? 1 : 0,
    pointerEvents: visible ? "auto" : "none",
    zIndex: visible ? 2 : 1,
  };
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
