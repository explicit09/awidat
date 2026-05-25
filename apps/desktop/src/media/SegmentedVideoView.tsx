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

import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { useMediaStore } from "./store";
import { cachedMediaStreamUrl, mediaStreamUrl } from "./mediaStreamUrl";
import {
  shouldRenderTransitionOnGpu,
  useGpuTransitionPreview,
} from "./useGpuTransitionPreview";
import { useProjectStore } from "../app/state";
import {
  useTimelineStore,
  type TimelineSnapshot,
} from "../timeline/store";
import type { TimelineParameterAnimation } from "../protocol";
import { clampOpacity, evaluateAnimations } from "../timeline/animation";
import { videoOverlayStyle as buildVideoOverlayStyle } from "./videoOverlayStyle";
import {
  findActiveSegment,
  findNextSegmentAfter,
  type PreviewTransition,
  type PlaySegment,
  usePreviewDuration,
  usePreviewTransitions,
  usePlaySegments,
  type VideoOverlaySegment,
  useVideoOverlaySegments,
} from "../timeline/usePlaySegments";

type SegmentedVideoViewProps = {
  chrome?: boolean;
  volume?: number;
  rate?: number;
};

export function SegmentedVideoView({
  chrome = true,
  volume = 1,
  rate = 1,
}: SegmentedVideoViewProps = {}) {
  const segments = usePlaySegments();
  const previewDurationS = usePreviewDuration();
  const timelineSnapshot = useTimelineStore((s) => s.snapshot);
  const transcodingCount = countClipsAwaitingProxy(timelineSnapshot);
  const setTimelineDuration = useMediaStore((s) => s.setTimelineDuration);
  const timelineDurationS = useTimelineStore((s) => s.snapshot.duration_s);
  const timelineTime = useMediaStore((s) => s.timelineTime);
  const requestTimelineSeek = useMediaStore((s) => s.requestTimelineSeek);

  // Mirror the snapshot duration into the media store so the scrub
  // bar can clamp without subscribing to the timeline store too.
  useEffect(() => {
    setTimelineDuration(previewDurationS || timelineDurationS);
  }, [previewDurationS, timelineDurationS, setTimelineDuration]);

  // Clamp the playhead when the timeline shrinks past it (e.g. user
  // deletes the clip the playhead is parked in). Without this the
  // SegmentedPlayer's findActiveSegment returns -1 and the player
  // looks frozen at "end of timeline" when it should snap to the
  // new end.
  useEffect(() => {
    if (timelineDurationS > 0 && timelineTime > timelineDurationS) {
      requestTimelineSeek(timelineDurationS);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [timelineDurationS]);

  if (segments.length === 0) {
    const awaitingProxy = transcodingCount > 0;
    return (
      <div className="video-wrap">
        <div className="video-stack">
          <div className="media-empty media-empty-stacked">
            <p className="media-empty-title">
              {awaitingProxy ? "Generating preview..." : "No playable clips yet"}
            </p>
            <p className="media-empty-hint">
              {awaitingProxy
                ? `${transcodingCount} clip${transcodingCount === 1 ? "" : "s"} waiting for proxy media.`
                : "Add a clip from the Media bin to start your timeline preview."}
            </p>
          </div>
        </div>
      </div>
    );
  }
  return <SegmentedPlayer segments={segments} chrome={chrome} volume={volume} rate={rate} />;
}

// One slot in the double-buffer.
type Slot = {
  ref: React.MutableRefObject<HTMLVideoElement | null>;
  /** Segment index this slot is loaded with, or -1 when empty. */
  segIdx: number;
};

function SegmentedPlayer({
  segments,
  chrome,
  volume,
  rate,
}: {
  segments: PlaySegment[];
  chrome: boolean;
  volume: number;
  rate: number;
}) {
  const videoOverlays = useVideoOverlaySegments();
  const previewTransitions = usePreviewTransitions();
  // For diagnostics: how many clips are on the OTIO but missing a
  // proxy (still transcoding). The user sees a "+ N transcoding…"
  // hint so they know the timeline isn't lying about its length.
  const transcodingCount = useTimelineStore((s) =>
    countClipsAwaitingProxy(s.snapshot),
  );
  const refA = useRef<HTMLVideoElement | null>(null);
  const refB = useRef<HTMLVideoElement | null>(null);
  const scrubInputRef = useRef<HTMLInputElement | null>(null);
  const scrubPointerIdRef = useRef<number | null>(null);
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
  const mediaError = useMediaStore((s) => s.mediaError);
  const setMediaError = useMediaStore((s) => s.setMediaError);
  const requestTimelineSeek = useMediaStore((s) => s.requestTimelineSeek);
  // Subscribe to the snapshot itself (Zustand caches the reference)
  // and derive the overlay list with useMemo. The previous shape
  // returned a fresh array from inside the selector on every render,
  // which tripped React's "getSnapshot should be cached" infinite-
  // loop guard.
  const timelineSnapshot = useTimelineStore((s) => s.snapshot);
  const projectRoot = useProjectStore((s) => s.current);
  const activeTitles = useMemo(
    () =>
      activeTitleOverlays(
        timelineSnapshot,
        timelineSnapshot.duration_s > 0 ? timelineSnapshot.duration_s : 0,
      ),
    [timelineSnapshot],
  );
  const activeShapes = useMemo(
    () => activeMotionShapeOverlays(timelineSnapshot),
    [timelineSnapshot],
  );
  const activeImages = useMemo(
    () => activeMotionImageOverlays(timelineSnapshot, projectRoot),
    [timelineSnapshot, projectRoot],
  );
  const activeVideoOverlays = useMemo(
    () =>
      videoOverlays.filter(
        (overlay) =>
          timelineTime >= overlay.timelineStart &&
          timelineTime < overlay.timelineEnd,
      ),
    [videoOverlays, timelineTime],
  );
  const activeTransition = useMemo(
    () =>
      previewTransitions.find(
        (transition) =>
          timelineTime >= transition.timelineStart &&
          timelineTime < transition.timelineEnd,
      ) ?? null,
    [previewTransitions, timelineTime],
  );

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

  // Reset both slots when the segments array identity changes (e.g.
  // an apply_edl landed). This must run before the resync effect
  // below; otherwise a freshly loaded active slot can be marked
  // empty after it starts playing, leaving timeline-time frozen.
  useEffect(() => {
    slotsRef.current.a.segIdx = -1;
    slotsRef.current.b.segIdx = -1;
  }, [segments]);

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
      const nextIdx = findNextSegmentAfter(segments, timelineTime);
      if (nextIdx >= 0) {
        const next = segments[nextIdx];
        ensureSlotLoaded(active, next);
        active.segIdx = nextIdx;
        applySegmentPlaybackSettings(v, next);
        if (Math.abs(timelineTime - next.timelineStart) > 0.001) {
          requestTimelineSeek(next.timelineStart);
        } else {
          tryAssignCurrentTime(v, next.sourceStart);
          playActiveSlotIfNeeded(active);
          primePreroll(nextIdx);
        }
        return;
      }
      // True end of timeline → pause + park at last frame.
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
    applySegmentPlaybackSettings(v, seg);
    const desired = seg.sourceStart + (timelineTime - seg.timelineStart);
    if (Math.abs(v.currentTime - desired) > 0.05) {
      tryAssignCurrentTime(v, desired);
    }
    // Preload the next segment into the inactive slot.
    primePreroll(segIdx);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [segments, timelineTime, seekRequestId, seekTargetS]);

  // Set src + park at sourceStart for a slot. Idempotent — if the
  // slot already has the right src loaded, we skip the assignment
  // (re-setting src reloads the decoder, which is exactly what
  // double-buffering is meant to avoid).
  function ensureSlotLoaded(slot: Slot, seg: PlaySegment) {
    const v = slot.ref.current;
    if (!v) return;
    const src = cachedMediaStreamUrl(seg.proxyPath);
    if (!src) {
      const wantPath = seg.proxyPath;
      mediaStreamUrl(wantPath)
        .then((url) => {
          if (slot.ref.current !== v) return;
          if (v.src !== url) v.src = url;
          setMediaError(null);
          alignSlotAfterLoad(slot === slotsRef.current.a ? "a" : "b");
          playActiveSlotIfNeeded(slot);
        })
        .catch((e) => {
          const message = `Could not open preview media: ${String(e)}`;
          setMediaError(message);
          console.warn("media stream url failed", e);
        });
      return;
    }
    if (v.src !== src) {
      v.src = src;
      setMediaError(null);
    }
    tryAssignCurrentTime(v, seg.sourceStart);
    playActiveSlotIfNeeded(slot);
  }

  function playActiveSlotIfNeeded(slot: Slot) {
    if (slot !== slotsRef.current[activeKeyRef.current]) return;
    if (!useMediaStore.getState().isPlaying) return;
    const v = slot.ref.current;
    if (!v || !v.paused) return;
    v.play().catch((err) => {
      handlePlayFailure(err, v);
    });
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
      applySegmentPlaybackSettings(v, next);
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
    if (vTime < seg.sourceStart - 0.05) {
      tryAssignCurrentTime(v, seg.sourceStart);
      return;
    }
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
      if (nextV) applySegmentPlaybackSettings(nextV, next);
      // Pause the leaving slot, play the entering slot.
      v.pause();
      if (nextV) {
        nextV.play().catch((err) => {
          handlePlayFailure(err, nextV);
        });
      }
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

  function alignSlotAfterLoad(key: "a" | "b") {
    const slot = slotsRef.current[key];
    const v = slot.ref.current;
    if (!v || slot.segIdx < 0) return;
    const seg = segmentsRef.current[slot.segIdx];
    if (!seg) return;
    const desired =
      key === activeKeyRef.current
        ? seg.sourceStart + (useMediaStore.getState().timelineTime - seg.timelineStart)
        : seg.sourceStart;
    if (Math.abs(v.currentTime - desired) > 0.05) {
      tryAssignCurrentTime(v, desired);
    }
    playActiveSlotIfNeeded(slot);
  }

  // Push view-state (~1Hz, integer-second granularity). Use the
  // *source* asset's stem so the agent's view-context line resolves
  // to a file under `raw/`. The proxy stem (e.g. `<id>-1080p-<hash>`)
  // would break tool calls like `view_frame` that look the stem up
  // against the source asset path.
  useEffect(() => {
    const segIdx = findActiveSegment(segments, timelineTime);
    if (segIdx < 0) return;
    const seg = segments[segIdx];
    const stem = seg.sourceStem ?? stemFromProxyPath(seg.proxyPath);
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
      v.play().catch((err) => {
        handlePlayFailure(err, v);
      });
    } else {
      v.pause();
    }
  }

  useEffect(() => {
    const v = slotsRef.current[activeKeyRef.current].ref.current;
    if (!v) return;
    if (isPlaying && v.paused) {
      v.play().catch((err) => {
        handlePlayFailure(err, v);
      });
    } else if (!isPlaying && !v.paused) {
      v.pause();
    }
  }, [activeKey, isPlaying, setMediaError, setPlaying]);

  function applySegmentPlaybackSettings(v: HTMLVideoElement, seg: PlaySegment) {
    const segmentVolume = Number.isFinite(seg.volume)
      ? Math.max(0, Math.min(1, seg.volume))
      : 1;
    const speed = Number.isFinite(seg.speed)
      ? Math.max(0.0625, Math.min(16, seg.speed))
      : 1;
    const effectiveVolume = Math.max(0, Math.min(1, segmentVolume * volume));
    const effectiveRate = Math.max(0.0625, Math.min(16, speed * rate));
    if (Math.abs(v.volume - effectiveVolume) > 0.001) v.volume = effectiveVolume;
    if (Math.abs(v.playbackRate - effectiveRate) > 0.001) v.playbackRate = effectiveRate;
  }

  useEffect(() => {
    const slot = slotsRef.current[activeKeyRef.current];
    const v = slot.ref.current;
    const seg = segmentsRef.current[slot.segIdx];
    if (v && seg) applySegmentPlaybackSettings(v, seg);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeKey, volume, rate]);

  function onVideoError(e: React.SyntheticEvent<HTMLVideoElement>) {
    const err = e.currentTarget.error;
    const code = err ? `code ${err.code}` : "unknown error";
    setMediaError(`Preview media failed to load (${code}).`);
  }

  function handlePlayFailure(err: unknown, v: HTMLVideoElement) {
    if (isAutoplayBlocked(err)) {
      window.setTimeout(() => {
        if (!v.paused) {
          setMediaError(null);
        } else {
          setPlaying(false);
        }
      }, 0);
      return;
    }
    setMediaError(`Playback failed: ${String(err)}`);
    setPlaying(false);
  }

  function onScrub(e: React.ChangeEvent<HTMLInputElement>) {
    const t = Number(e.target.value);
    if (Number.isFinite(t)) requestTimelineSeek(t);
  }

  function onScrubInput(e: React.FormEvent<HTMLInputElement>) {
    const t = Number(e.currentTarget.value);
    if (Number.isFinite(t)) requestTimelineSeek(t);
  }

  function seekFromScrubPointer(
    el: HTMLInputElement,
    clientX: number,
  ) {
    if (timelineDurationS <= 0) return;
    const rect = el.getBoundingClientRect();
    const ratio =
      rect.width > 0
        ? Math.max(0, Math.min(1, (clientX - rect.left) / rect.width))
        : 0;
    requestTimelineSeek(ratio * timelineDurationS);
  }

  function onScrubPointerDown(e: React.PointerEvent<HTMLInputElement>) {
    if (timelineDurationS <= 0) return;
    scrubInputRef.current = e.currentTarget;
    scrubPointerIdRef.current = e.pointerId;
    e.currentTarget.setPointerCapture(e.pointerId);
    seekFromScrubPointer(e.currentTarget, e.clientX);
  }

  function onScrubPointerMove(e: React.PointerEvent<HTMLInputElement>) {
    if (scrubPointerIdRef.current !== e.pointerId) return;
    seekFromScrubPointer(e.currentTarget, e.clientX);
  }

  function finishScrubPointer(pointerId: number) {
    if (scrubPointerIdRef.current !== pointerId) return;
    const el = scrubInputRef.current;
    if (el?.hasPointerCapture(pointerId)) {
      el.releasePointerCapture(pointerId);
    }
    scrubPointerIdRef.current = null;
    scrubInputRef.current = null;
  }

  useEffect(() => {
    function onWindowPointerMove(e: PointerEvent) {
      if (scrubPointerIdRef.current !== e.pointerId) return;
      const el = scrubInputRef.current;
      if (el) seekFromScrubPointer(el, e.clientX);
    }
    function onWindowPointerUp(e: PointerEvent) {
      finishScrubPointer(e.pointerId);
    }
    window.addEventListener("pointermove", onWindowPointerMove);
    window.addEventListener("pointerup", onWindowPointerUp);
    window.addEventListener("pointercancel", onWindowPointerUp);
    return () => {
      window.removeEventListener("pointermove", onWindowPointerMove);
      window.removeEventListener("pointerup", onWindowPointerUp);
      window.removeEventListener("pointercancel", onWindowPointerUp);
    };
  });

  // The hidden slot uses opacity 0 + pointer-events: none so it
  // doesn't intercept clicks on the visible video. Both elements
  // are positioned absolutely in the same box; the visible slot
  // has opacity 1 and z-index above. We avoid `display: none` and
  // `visibility: hidden` because both can pause buffering on some
  // browsers.
  const activeSlotOpacity = activeTransition
    ? baseTransitionOpacity(
        activeTransition.kind,
        transitionProgress(activeTransition, timelineTime),
        timelineTime < activeTransition.cutTime ? "outgoing" : "incoming",
      )
    : 1;
  const styleA = useMemo(
    () => slotStyle(activeKey === "a", activeKey === "a" ? activeSlotOpacity : 0),
    [activeKey, activeSlotOpacity],
  );
  const styleB = useMemo(
    () => slotStyle(activeKey === "b", activeKey === "b" ? activeSlotOpacity : 0),
    [activeKey, activeSlotOpacity],
  );

  const stackRef = useRef<HTMLDivElement | null>(null);
  const [stackSize, setStackSize] = useState<{ width: number; height: number }>({
    width: 0,
    height: 0,
  });
  useLayoutEffect(() => {
    const el = stackRef.current;
    if (!el) return;
    const update = () => {
      setStackSize({ width: el.clientWidth, height: el.clientHeight });
    };
    update();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => update());
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  return (
    <div className="video-wrap">
      <div className="video-stack" ref={stackRef}>
        {mediaError && <MediaErrorOverlay message={mediaError} />}
        <video
          ref={refA}
          className="video-el"
          preload="auto"
          style={styleA}
          onTimeUpdate={activeKey === "a" ? onTimeUpdate : undefined}
          onLoadedMetadata={() => alignSlotAfterLoad("a")}
          onCanPlay={() => {
            setMediaError(null);
            alignSlotAfterLoad("a");
          }}
          onError={onVideoError}
          onPlay={
            activeKey === "a"
              ? () => {
                  setMediaError(null);
                  setPlaying(true);
                }
              : undefined
          }
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
          onLoadedMetadata={() => alignSlotAfterLoad("b")}
          onCanPlay={() => {
            setMediaError(null);
            alignSlotAfterLoad("b");
          }}
          onError={onVideoError}
          onPlay={
            activeKey === "b"
              ? () => {
                  setMediaError(null);
                  setPlaying(true);
                }
              : undefined
          }
          onPause={activeKey === "b" ? () => setPlaying(false) : undefined}
          onEnded={activeKey === "b" ? () => setPlaying(false) : undefined}
          onClick={activeKey === "b" ? togglePlay : undefined}
        />
        <TimelineVideoOverlays
          overlays={activeVideoOverlays}
          timelineTime={timelineTime}
          isPlaying={isPlaying}
        />
        <TimelineTransitionOverlay
          transition={
            shouldRenderTransitionOnGpu(activeTransition) ? null : activeTransition
          }
          timelineTime={timelineTime}
          isPlaying={isPlaying}
        />
        <TimelineTransitionColorOverlay
          transition={
            shouldRenderTransitionOnGpu(activeTransition) ? null : activeTransition
          }
          timelineTime={timelineTime}
        />
        <GpuTransitionPreview
          transition={activeTransition}
          timelineTime={timelineTime}
          width={stackSize.width}
          height={stackSize.height}
        />
        <TimelineTitleOverlays
          overlays={activeTitles}
          timelineTime={timelineTime}
        />
        <TimelineMotionShapeOverlays
          overlays={activeShapes}
          timelineTime={timelineTime}
        />
        <TimelineMotionImageOverlays
          overlays={activeImages}
          timelineTime={timelineTime}
        />
        <TimelineBroadcastOverlay
          overlay={timelineSnapshot.broadcast_overlay}
          timelineTime={timelineTime}
          projectRoot={projectRoot}
        />
      </div>
      {chrome ? (
        <>
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
              onInput={onScrubInput}
              onPointerDown={onScrubPointerDown}
              onPointerMove={onScrubPointerMove}
              onPointerUp={(e) => finishScrubPointer(e.pointerId)}
              onPointerCancel={(e) => finishScrubPointer(e.pointerId)}
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
              {transcodingCount > 0
                ? ` · +${transcodingCount} transcoding…`
                : ""}
            </code>
          </div>
        </>
      ) : null}
    </div>
  );
}

function TimelineTransitionOverlay({
  transition,
  timelineTime,
  isPlaying,
}: {
  transition: PreviewTransition | null;
  timelineTime: number;
  isPlaying: boolean;
}) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
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
    const elapsed = timelineTime - transition.timelineStart;
    const sourceTime =
      overlaySide === "incoming"
        ? transition.to.sourceStart - transition.inOffset + elapsed
        : transition.from.sourceEnd - transition.inOffset + elapsed;
    if (Math.abs(v.currentTime - sourceTime) > 0.05) {
      tryAssignCurrentTime(v, sourceTime);
    }
    const speed = Number.isFinite(overlaySegment.speed)
      ? Math.max(0.0625, Math.min(16, overlaySegment.speed))
      : 1;
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
function GpuTransitionPreview({
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

function TimelineTransitionColorOverlay({
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

function transitionProgress(
  transition: PreviewTransition,
  timelineTime: number,
): number {
  if (transition.duration <= 0) return 1;
  return Math.max(
    0,
    Math.min(1, (timelineTime - transition.timelineStart) / transition.duration),
  );
}

function baseTransitionOpacity(
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
  return kind === "SMPTE_Dissolve" || kind === "awidat.cross_dissolve" || kind === "fade";
}

function isFadeThroughBlack(kind: string): boolean {
  return (
    kind === "awidat.fade_black" ||
    kind === "fadeblack" ||
    kind === "awidat.fade_in" ||
    kind === "awidat.fade_out"
  );
}

function isFlashWhite(kind: string): boolean {
  return kind === "awidat.flash_white" || kind === "fadewhite";
}

function isSlideLeft(kind: string): boolean {
  return (
    kind === "awidat.slide_left" ||
    kind === "awidat.smooth_push_left" ||
    kind === "slideleft" ||
    kind === "smoothleft"
  );
}

function isSlideRight(kind: string): boolean {
  return kind === "awidat.slide_right" || kind === "slideright" || kind === "smoothright";
}

function isWipeLeft(kind: string): boolean {
  return kind === "awidat.wipe_left" || kind === "wipeleft";
}

function isWipeRight(kind: string): boolean {
  return kind === "awidat.wipe_right" || kind === "wiperight";
}

function isZoomIn(kind: string): boolean {
  return kind === "awidat.zoom_in" || kind === "zoomin";
}

function isRadial(kind: string): boolean {
  return kind === "awidat.radial" || kind === "radial";
}

function isPixelize(kind: string): boolean {
  return kind === "awidat.pixelize" || kind === "pixelize";
}

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
    const desired = overlay.sourceStart + (timelineTime - overlay.timelineStart);
    if (Number.isFinite(desired) && Math.abs(v.currentTime - desired) > 0.08) {
      tryAssignCurrentTime(v, desired);
    }
    const speed = Number.isFinite(overlay.speed)
      ? Math.max(0.0625, Math.min(16, overlay.speed))
      : 1;
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

type BroadcastOverlayConfig = NonNullable<TimelineSnapshot["broadcast_overlay"]>;
type BroadcastHost = BroadcastOverlayConfig["host_a"];
type BroadcastTimedEntry = BroadcastOverlayConfig["topics"][number];

export function TimelineBroadcastOverlay({
  overlay,
  timelineTime,
  projectRoot,
  resolveAssetUrl = projectAssetUrl,
}: {
  overlay: TimelineSnapshot["broadcast_overlay"];
  timelineTime: number;
  projectRoot: string | null;
  resolveAssetUrl?: (projectRoot: string | null, relPath: string | null) => string | null;
}) {
  if (!overlay?.enabled) return null;

  const style = overlay.style;
  const gold = normalizeCssHex(style.gold_hex, "#C9A028");
  const goldLight = normalizeCssHex(style.gold_light_hex, "#E8C040");
  const cyan = normalizeCssHex(style.cyan_hex, "#22D3EE");
  const navy = normalizeCssHex(style.dark_navy_hex, "#070D17");
  const inTitle = timelineTime >= 0 && timelineTime < style.title_visible_end;
  const inHostIntro =
    timelineTime >= style.host_intro_start && timelineTime < style.host_intro_end;
  const tickerEntries = broadcastTickerEntries(overlay);
  const activeChapter =
    overlay.chapters.length > 0
      ? activeChapterEntry(
          overlay.chapters,
          timelineTime,
          style.chapter_display_duration,
          Math.max(0, style.title_visible_end),
        )
      : null;
  const tickerPhase = broadcastTickerPhase(
    tickerEntries,
    timelineTime,
    style,
  );
  const sponsorText =
    overlay.sponsors.length > 0
      ? `${overlay.sponsors.join("   ◆   ")}   ◆`
      : overlay.show_name || overlay.template_name || "BROADCAST";
  const overlayStyleVars = {
    "--broadcast-name-bar-height": refHeightPercent(style.name_bar_height),
    "--broadcast-ticker-height": refHeightPercent(style.ticker_height),
    "--broadcast-host-strip-height": refHeightPercent(style.host_strip_height),
    "--broadcast-ticker-label-width": refWidthPercent(680),
  } as React.CSSProperties;

  return (
    <div className="broadcast-overlay-layer" style={overlayStyleVars} aria-hidden="true">
      <BroadcastAssetPreloads
        overlay={overlay}
        projectRoot={projectRoot}
        resolveAssetUrl={resolveAssetUrl}
      />
      {overlay.short_form_mode ? (
        <div
          className="broadcast-short-brand-bar"
          style={{
            "--broadcast-navy": navy,
            "--broadcast-gold": gold,
          } as React.CSSProperties}
        >
          <BroadcastBrandLogo
            logoPath={overlay.brand_logo_path}
            projectRoot={projectRoot}
            resolveAssetUrl={resolveAssetUrl}
          />
          <strong>{(overlay.show_name || overlay.episode_title || "BROADCAST").toUpperCase()}</strong>
        </div>
      ) : (
        <>
      {inTitle && (
        <div
          className="broadcast-title-card"
          style={{
            "--broadcast-navy": navy,
            "--broadcast-gold": gold,
            "--broadcast-cyan": cyan,
            opacity: titleCardOpacity(timelineTime, style),
          } as React.CSSProperties}
        >
          <div className="broadcast-title-eyebrow">EPISODE</div>
          <div className="broadcast-title-main">
            {(overlay.episode_title || overlay.show_name).toUpperCase()}
          </div>
          {overlay.episode_subtitle && (
            <div className="broadcast-title-subtitle">
              {overlay.episode_subtitle}
            </div>
          )}
        </div>
      )}

      {inHostIntro ? (
        <div
          className="broadcast-host-intro-strip"
          style={{
            "--broadcast-gold": gold,
            "--broadcast-gold-light": goldLight,
            "--broadcast-navy": navy,
          } as React.CSSProperties}
        >
          <BroadcastIntroHost
            host={overlay.host_a}
            projectRoot={projectRoot}
            resolveAssetUrl={resolveAssetUrl}
          />
          <div className="broadcast-host-intro-divider" />
          <BroadcastIntroHost
            host={overlay.host_b}
            projectRoot={projectRoot}
            resolveAssetUrl={resolveAssetUrl}
            align="right"
          />
        </div>
      ) : (
        <>
          <div
            className="broadcast-name-bar"
            style={{
              "--broadcast-navy": navy,
              "--broadcast-gold": gold,
            } as React.CSSProperties}
          >
            <BroadcastName host={overlay.host_a} />
            <div className="broadcast-name-divider" />
            <BroadcastName host={overlay.host_b} align="right" />
          </div>
          <div
            className="broadcast-ticker"
            style={{
              "--broadcast-navy": navy,
              "--broadcast-gold": gold,
              "--broadcast-cyan": cyan,
            } as React.CSSProperties}
          >
            <div className="broadcast-ticker-show">
              {(overlay.show_name || "BROADCAST").toUpperCase()}
            </div>
            <div className="broadcast-ticker-content">
              <BroadcastSponsorMarquee
                sponsorText={sponsorText}
                timelineTime={timelineTime}
                opacity={tickerPhase.sponsorOpacity}
              />
              {tickerPhase.activeTopic && (
                <div
                  className="broadcast-topic"
                  style={{ opacity: tickerPhase.topicOpacity }}
                >
                  <span>NOW DISCUSSING</span>
                  <strong>{tickerPhase.activeTopic.text}</strong>
                </div>
              )}
            </div>
          </div>
        </>
      )}

      {activeChapter && (
        <div
          className="broadcast-chapter-card"
          style={{
            "--broadcast-navy": navy,
            "--broadcast-gold": gold,
          } as React.CSSProperties}
        >
          <span>{chapterNumber(overlay.chapters, activeChapter)}</span>
          <strong>{activeChapter.text.toUpperCase()}</strong>
        </div>
      )}
        </>
      )}
    </div>
  );
}

function BroadcastAssetPreloads({
  overlay,
  projectRoot,
  resolveAssetUrl,
}: {
  overlay: BroadcastOverlayConfig;
  projectRoot: string | null;
  resolveAssetUrl: (projectRoot: string | null, relPath: string | null) => string | null;
}) {
  const urls = [
    resolveAssetUrl(projectRoot, overlay.brand_logo_path),
    resolveAssetUrl(projectRoot, overlay.host_a.photo_path),
    resolveAssetUrl(projectRoot, overlay.host_b.photo_path),
  ].filter((url): url is string => Boolean(url));
  if (urls.length === 0) return null;
  return (
    <div className="broadcast-asset-preloads">
      {urls.map((url) => (
        <img key={url} src={url} alt="" />
      ))}
    </div>
  );
}

function BroadcastSponsorMarquee({
  sponsorText,
  timelineTime,
  opacity,
}: {
  sponsorText: string;
  timelineTime: number;
  opacity: number;
}) {
  const segmentRef = useRef<HTMLSpanElement | null>(null);
  const [segmentWidth, setSegmentWidth] = useState(0);

  useLayoutEffect(() => {
    const element = segmentRef.current;
    if (!element) return;
    const measure = () => setSegmentWidth(element.getBoundingClientRect().width);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [sponsorText]);

  const scrollPxPerSecond = 48 * (segmentWidth > 0 ? segmentWidth / sponsorTextReferenceWidth(sponsorText) : 0);
  const offset =
    segmentWidth > 0 ? (timelineTime * scrollPxPerSecond) % segmentWidth : 0;

  return (
    <div className="broadcast-sponsor-marquee" style={{ opacity }}>
      <div
        className="broadcast-sponsor-track"
        style={{ transform: `translate3d(${-offset}px, 0, 0)` }}
      >
        <span ref={segmentRef} className="broadcast-sponsor-segment">
          {sponsorText}
        </span>
        <span className="broadcast-sponsor-segment">{sponsorText}</span>
        <span className="broadcast-sponsor-segment">{sponsorText}</span>
      </div>
    </div>
  );
}

function sponsorTextReferenceWidth(text: string): number {
  return Math.max(1, text.length * 28);
}

function BroadcastBrandLogo({
  logoPath,
  projectRoot,
  resolveAssetUrl,
}: {
  logoPath: string | null;
  projectRoot: string | null;
  resolveAssetUrl: (projectRoot: string | null, relPath: string | null) => string | null;
}) {
  const logo = resolveAssetUrl(projectRoot, logoPath);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    setFailed(false);
  }, [logo]);
  if (!logo || failed) return null;
  return <img src={logo} alt="" onError={() => setFailed(true)} />;
}

function BroadcastName({
  host,
  align,
}: {
  host: BroadcastHost;
  align?: "right";
}) {
  if (!host.name.trim()) return <div />;
  return (
    <div className={`broadcast-name ${align === "right" ? "align-right" : ""}`}>
      <strong>{host.name.toUpperCase()}</strong>
      {host.title && <span>{host.title.toUpperCase()}</span>}
    </div>
  );
}

function BroadcastIntroHost({
  host,
  projectRoot,
  resolveAssetUrl,
  align,
}: {
  host: BroadcastHost;
  projectRoot: string | null;
  resolveAssetUrl: (projectRoot: string | null, relPath: string | null) => string | null;
  align?: "right";
}) {
  const photo = resolveAssetUrl(projectRoot, host.photo_path);
  const [photoFailed, setPhotoFailed] = useState(false);
  useEffect(() => {
    setPhotoFailed(false);
  }, [photo]);
  if (!host.name.trim()) return <div />;
  const initials = host.name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0]?.toUpperCase())
    .join("");
  return (
    <div className={`broadcast-intro-host ${align === "right" ? "align-right" : ""}`}>
      <div className="broadcast-host-photo">
        {photo && !photoFailed ? (
          <img src={photo} alt="" onError={() => setPhotoFailed(true)} />
        ) : (
          <span>{initials}</span>
        )}
      </div>
      <div>
        <strong>{host.name.toUpperCase()}</strong>
        {host.title && <span>{host.title.toUpperCase()}</span>}
      </div>
    </div>
  );
}

function activeChapterEntry(
  entries: BroadcastTimedEntry[],
  timelineTime: number,
  duration: number,
  minStart: number,
): BroadcastTimedEntry | null {
  let active: BroadcastTimedEntry | null = null;
  for (const entry of entries) {
    const start = Math.max(minStart, entry.time_seconds);
    const end = start + Math.max(0.25, duration);
    if (timelineTime >= start && timelineTime < end) active = entry;
  }
  return active;
}

function broadcastTickerEntries(
  overlay: BroadcastOverlayConfig,
): BroadcastTimedEntry[] {
  return overlay.topics.length > 0 ? overlay.topics : overlay.chapters;
}

function broadcastTickerPhase(
  entries: BroadcastTimedEntry[],
  timelineTime: number,
  style: BroadcastOverlayConfig["style"],
): {
  activeTopic: BroadcastTimedEntry | null;
  sponsorOpacity: number;
  topicOpacity: number;
} {
  const topic = [...entries].reverse().find((entry) => entry.time_seconds <= timelineTime);
  if (!topic) {
    return { activeTopic: null, sponsorOpacity: 1, topicOpacity: 0 };
  }
  const sponsor = Math.max(0, style.ticker_sponsor_duration);
  const fade = Math.max(0, style.ticker_fade_duration);
  const topicDuration = Math.max(0.25, style.ticker_topic_duration);
  const cycle = sponsor + fade + topicDuration + fade;
  if (cycle <= 0) return { activeTopic: topic, sponsorOpacity: 0, topicOpacity: 1 };
  const cyclePos = ((timelineTime % cycle) + cycle) % cycle;
  if (cyclePos < sponsor - fade) {
    return { activeTopic: topic, sponsorOpacity: 1, topicOpacity: 0 };
  }
  if (cyclePos < sponsor) {
    const topicOpacity = fade <= 0 ? 1 : (cyclePos - (sponsor - fade)) / fade;
    return {
      activeTopic: topic,
      sponsorOpacity: 1 - topicOpacity,
      topicOpacity,
    };
  }
  if (cyclePos < sponsor + topicDuration) {
    return { activeTopic: topic, sponsorOpacity: 0, topicOpacity: 1 };
  }
  if (cyclePos < sponsor + topicDuration + fade) {
    const sponsorOpacity = fade <= 0 ? 1 : (cyclePos - sponsor - topicDuration) / fade;
    return {
      activeTopic: topic,
      sponsorOpacity,
      topicOpacity: 1 - sponsorOpacity,
    };
  }
  return { activeTopic: topic, sponsorOpacity: 1, topicOpacity: 0 };
}

function chapterNumber(
  chapters: BroadcastTimedEntry[],
  active: BroadcastTimedEntry,
): string {
  const index = chapters.findIndex((chapter) => chapter === active);
  return String(index >= 0 ? index + 1 : 1);
}

function titleCardOpacity(
  t: number,
  style: BroadcastOverlayConfig["style"],
): number {
  const fadeIn = Math.max(0.001, style.title_fade_in_end);
  const fadeOutStart = style.title_fade_out_start;
  const end = Math.max(fadeOutStart + 0.001, style.title_visible_end);
  if (t < fadeIn) return Math.max(0, Math.min(1, t / fadeIn));
  if (t < fadeOutStart) return 1;
  return Math.max(0, Math.min(1, (end - t) / (end - fadeOutStart)));
}

function normalizeCssHex(value: string, fallback: string): string {
  if (!value.trim()) return fallback;
  return value.startsWith("#") ? value : `#${value}`;
}

function refHeightPercent(value: number): string {
  return `${(value / 2160) * 100}%`;
}

function refWidthPercent(value: number): string {
  return `${(value / 3840) * 100}%`;
}

function projectAssetUrl(projectRoot: string | null, relPath: string | null): string | null {
  if (!projectRoot || !relPath) return null;
  if (relPath.startsWith("/") || relPath.includes("..")) return null;
  const root = projectRoot.endsWith("/") ? projectRoot.slice(0, -1) : projectRoot;
  return convertFileSrc(`${root}/${relPath}`);
}

type PreviewTitleOverlay = {
  key: string;
  startS: number;
  endS: number;
  text: string;
  position: "top" | "center" | "bottom";
  fontSize: number;
  color: string;
  fontWeight: "normal" | "bold";
  animation: "none" | "fade_in" | "fade_out" | "fade_in_out" | "slide_in" | "slide_out";
  reveal: "none" | "typewriter" | "word" | "line";
  animations: TimelineParameterAnimation[];
};

type PreviewMotionShapeOverlay = {
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

type PreviewMotionImageOverlay = {
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

function activeTitleOverlays(
  snapshot: TimelineSnapshot,
  _durationS: number,
): PreviewTitleOverlay[] {
  if (broadcastOverlayOwnsProgramTitles(snapshot.broadcast_overlay)) return [];

  const titleTrack = snapshot.tracks.find((track) => track.role === "titles");
  if (!titleTrack) return [];

  const overlays: PreviewTitleOverlay[] = [];
  for (const item of titleTrack.items) {
    if (item.kind !== "clip" || item.title === null) continue;
    const startS = item.track_start_s;
    const endS = item.track_start_s + item.duration_s;
    if (!Number.isFinite(startS) || !Number.isFinite(endS) || endS <= startS) {
      continue;
    }
    overlays.push({
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
    });
  }
  return overlays;
}

function activeMotionShapeOverlays(
  snapshot: TimelineSnapshot,
): PreviewMotionShapeOverlay[] {
  const overlays: PreviewMotionShapeOverlay[] = [];
  for (const track of snapshot.tracks) {
    for (const item of track.items) {
      if (item.kind !== "clip" || item.motion_shape === null) continue;
      if (item.motion_shape.shape !== "rect") continue;
      const startS = item.track_start_s;
      const endS = item.track_start_s + item.duration_s;
      if (!Number.isFinite(startS) || !Number.isFinite(endS) || endS <= startS) {
        continue;
      }
      overlays.push({
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
      });
    }
  }
  return overlays;
}

function activeMotionImageOverlays(
  snapshot: TimelineSnapshot,
  projectRoot: string | null,
): PreviewMotionImageOverlay[] {
  const overlays: PreviewMotionImageOverlay[] = [];
  for (const track of snapshot.tracks) {
    for (const item of track.items) {
      if (item.kind !== "clip" || item.motion_image === null) continue;
      const src = projectAssetUrl(projectRoot, item.motion_image.asset_id);
      if (src === null) continue;
      const startS = item.track_start_s;
      const endS = item.track_start_s + item.duration_s;
      if (!Number.isFinite(startS) || !Number.isFinite(endS) || endS <= startS) {
        continue;
      }
      overlays.push({
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
      });
    }
  }
  return overlays;
}

function broadcastOverlayOwnsProgramTitles(
  overlay: TimelineSnapshot["broadcast_overlay"],
): boolean {
  return Boolean(overlay?.enabled && !overlay.short_form_mode);
}

function TimelineTitleOverlays({
  overlays,
  timelineTime,
}: {
  overlays: PreviewTitleOverlay[];
  timelineTime: number;
}) {
  const active = overlays.filter(
    (overlay) => timelineTime >= overlay.startS && timelineTime < overlay.endS,
  );
  if (active.length === 0) return null;
  return (
    <div className="timeline-title-layer" aria-hidden="true">
      {active.map((overlay) => (
        <div
          key={overlay.key}
          className={`timeline-title-overlay title-pos-${overlay.position}`}
          style={titleOverlayStyle(overlay, timelineTime)}
        >
          {titleRevealText(overlay, timelineTime)}
        </div>
      ))}
    </div>
  );
}

function TimelineMotionShapeOverlays({
  overlays,
  timelineTime,
}: {
  overlays: PreviewMotionShapeOverlay[];
  timelineTime: number;
}) {
  const active = overlays.filter(
    (overlay) => timelineTime >= overlay.startS && timelineTime < overlay.endS,
  );
  if (active.length === 0) return null;
  return (
    <div className="timeline-motion-shape-layer" aria-hidden="true">
      {active.map((overlay) => (
        <div
          key={overlay.key}
          className="timeline-motion-shape-rect"
          style={motionShapeOverlayStyle(overlay, timelineTime)}
        />
      ))}
    </div>
  );
}

function TimelineMotionImageOverlays({
  overlays,
  timelineTime,
}: {
  overlays: PreviewMotionImageOverlay[];
  timelineTime: number;
}) {
  const active = overlays.filter(
    (overlay) => timelineTime >= overlay.startS && timelineTime < overlay.endS,
  );
  if (active.length === 0) return null;
  return (
    <div className="timeline-motion-image-layer" aria-hidden="true">
      {active.map((overlay) => (
        <img
          key={overlay.key}
          className="timeline-motion-image"
          src={overlay.src}
          style={motionImageOverlayStyle(overlay, timelineTime)}
        />
      ))}
    </div>
  );
}

function motionShapeOverlayStyle(
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

function motionImageOverlayStyle(
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

function motionImageFit(value: string): "cover" | "contain" | "fill" {
  if (value === "contain") return "contain";
  if (value === "stretch") return "fill";
  return "cover";
}

function titleOverlayStyle(
  overlay: PreviewTitleOverlay,
  timelineTime: number,
): React.CSSProperties {
  const elapsed = timelineTime - overlay.startS;
  const remaining = overlay.endS - timelineTime;
  const fadeIn = Math.min(1, Math.max(0, elapsed / 0.45));
  const fadeOut = Math.min(1, Math.max(0, remaining / 0.45));
  let opacity = 1;
  if (overlay.animation === "fade_in") opacity = fadeIn;
  if (overlay.animation === "fade_out") opacity = fadeOut;
  if (overlay.animation === "fade_in_out") opacity = Math.min(fadeIn, fadeOut);

  let translateX = "-50%";
  if (overlay.animation === "slide_in" && elapsed < 0.55) {
    const p = Math.min(1, Math.max(0, elapsed / 0.55));
    translateX = `calc(-50% + ${(1 - p) * -18}%)`;
  } else if (overlay.animation === "slide_out" && remaining < 0.55) {
    const p = Math.min(1, Math.max(0, remaining / 0.55));
    translateX = `calc(-50% + ${(1 - p) * 18}%)`;
  }
  const translateY = overlay.position === "center" ? "-50%" : "0";
  const animated = evaluateAnimations(overlay.animations, elapsed);
  if (animated["title.opacity"] !== undefined) {
    opacity = clampOpacity(animated["title.opacity"]);
  }
  const fontSize = animated["title.font_size"] ?? overlay.fontSize;
  const xOffset = animated["title.x"] ?? 0;
  const yOffset = animated["title.y"] ?? 0;

  return {
    color: overlay.color,
    fontSize: `clamp(15px, ${Math.max(1.2, fontSize / 22).toFixed(2)}vw, ${fontSize}px)`,
    fontWeight: overlay.fontWeight === "bold" ? 750 : 500,
    opacity,
    transform: `translate(calc(${translateX} + ${xOffset * 100}vw), calc(${translateY} + ${yOffset * 100}vh))`,
  };
}

function titlePosition(value: string): PreviewTitleOverlay["position"] {
  return value === "top" || value === "bottom" ? value : "center";
}

function titleAnimation(value: string): PreviewTitleOverlay["animation"] {
  switch (value) {
    case "fade_in":
    case "fade_out":
    case "fade_in_out":
    case "slide_in":
    case "slide_out":
      return value;
    default:
      return "none";
  }
}

function titleReveal(value: string): PreviewTitleOverlay["reveal"] {
  switch (value) {
    case "typewriter":
    case "word":
    case "line":
      return value;
    default:
      return "none";
  }
}

function titleRevealText(overlay: PreviewTitleOverlay, timelineTime: number): string {
  if (overlay.reveal === "none") return overlay.text;
  const elapsed = Math.max(0, timelineTime - overlay.startS);
  const duration = Math.max(0.001, overlay.endS - overlay.startS);
  const progress = Math.min(1, elapsed / duration);
  const steps = revealSteps(overlay.text, overlay.reveal);
  if (steps.length === 0) return "";
  const index = Math.min(steps.length - 1, Math.floor(progress * steps.length));
  return steps[index];
}

function revealSteps(text: string, reveal: PreviewTitleOverlay["reveal"]): string[] {
  if (reveal === "typewriter") {
    return Array.from(text).map((_, index, chars) => chars.slice(0, index + 1).join(""));
  }
  if (reveal === "word") {
    const matches = [...text.matchAll(/\S+/g)];
    return matches.map((match) => text.slice(0, (match.index ?? 0) + match[0].length));
  }
  if (reveal === "line") {
    const lines = text.match(/.*(?:\n|$)/g)?.filter((line) => line.length > 0) ?? [];
    let cursor = 0;
    return lines.map((line) => {
      cursor += line.length;
      return text.slice(0, cursor);
    });
  }
  return [text];
}

/**
 * Count clips on any video track that reference an asset but have
 * `proxy_path === null` — i.e. the transcoder hasn't finished
 * generating their preview proxy yet. Surfaces as a "+ N
 * transcoding…" hint in the meta strip so the user knows why the
 * timeline preview is shorter than the timeline ruler.
 */
function countClipsAwaitingProxy(snapshot: TimelineSnapshot): number {
  let n = 0;
  for (const track of snapshot.tracks) {
    if (track.kind !== "video") continue;
    for (const item of track.items) {
      if (
        item.kind === "clip" &&
        item.asset_id !== null &&
        item.proxy_path === null &&
        item.playable_kind === "source" &&
        item.duration_s > 0
      ) {
        n += 1;
      }
    }
  }
  return n;
}

function slotStyle(visible: boolean, opacity: number): React.CSSProperties {
  return {
    position: "absolute",
    inset: 0,
    width: "100%",
    height: "100%",
    opacity: visible ? opacity : 0,
    pointerEvents: visible ? "auto" : "none",
    zIndex: visible ? 2 : 1,
  };
}

function MediaErrorOverlay({ message }: { message: string }) {
  return (
    <div className="media-error-overlay">
      <p className="media-error-title">Preview unavailable</p>
      <p className="media-error-message">{message}</p>
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

function isAutoplayBlocked(err: unknown): boolean {
  if (err instanceof DOMException && err.name === "NotAllowedError") return true;
  return String(err).includes("NotAllowedError");
}
