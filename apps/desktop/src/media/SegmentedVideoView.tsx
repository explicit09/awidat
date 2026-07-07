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
// Same-asset back-to-back segments (the montage common case) get
// minor benefit since the proxy is already cached; cross-asset
// crossings get the full benefit (no flash).
//
// Architecture:
//
//   monotonic timeline clock ──→ timelineTime ──→ active segment ──→ media
//
// Media elements are adapters. They follow timeline time and are only
// hard-sought on external seeks, segment entry, pause, or large drift.
// This avoids the seek/stall/drift loop caused by using playing video
// elements as the master clock.

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { useMediaStore } from "./store";
import { cachedMediaStreamUrl, mediaStreamUrl } from "./mediaStreamUrl";
import {
  SHUTTLE_STEP_MS,
  driftRecoveryAction,
  fadeGainMultiplier,
  isShuttleRate,
  timelineTimeForSegmentPosition,
} from "./previewHandoff";
import {
  resumePreviewAudio,
  setPreviewElementGain,
} from "./previewAudioGraph";
import { colorPreviewCssFilter } from "./colorPreviewFilter";
import { GradeCanvas } from "./GradeCanvas";
import { useColorPreviewOverride } from "../properties/store";
import { shouldRenderTransitionOnGpu } from "./useGpuTransitionPreview";
import { containedProgramFrame, programFrameStyle } from "./programFrame";
import { useProjectStore } from "../app/state";
import {
  useTimelineStore,
  type TimelineSnapshot,
} from "../timeline/store";
import { baseTransitionOpacity, transitionProgress } from "./stage/transitions";
import { activeTitleOverlays } from "./stage/titles";
import { activeMotionSceneOverlays } from "./stage/motionScene";
import { livePreviewClock } from "./stage/stageClock";
import { Stage } from "./stage/Stage";
import {
  findActiveSegment,
  findNextSegmentAfter,
  type PlaySegment,
  safeSegmentSpeed,
  shouldAutoAdvanceTimelineGap,
  sourceTimeForTimelineTime,
  usePreviewDuration,
  usePreviewTransitions,
  usePlaySegments,
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

type PreviewClock = {
  baseTime: number;
  playStartMs: number | null;
  rate: number;
  duration: number;
};

function safePlaybackRate(rate: number): number {
  return Number.isFinite(rate) && rate > 0 ? Math.max(0.1, Math.min(5, rate)) : 1;
}

function previewClockNow(clock: PreviewClock): number {
  if (clock.playStartMs === null) return clock.baseTime;
  const elapsedS = ((performance.now() - clock.playStartMs) / 1000) * clock.rate;
  const next = Math.max(0, clock.baseTime + elapsedS);
  return clock.duration > 0 ? Math.min(next, clock.duration) : next;
}

function previewClockSeek(clock: PreviewClock, timeS: number) {
  const clamped = clock.duration > 0 ? Math.min(timeS, clock.duration) : timeS;
  clock.baseTime = Math.max(0, Number.isFinite(clamped) ? clamped : 0);
  if (clock.playStartMs !== null) clock.playStartMs = performance.now();
}

function previewClockPlay(clock: PreviewClock) {
  if (clock.playStartMs !== null) return;
  if (clock.duration > 0 && clock.baseTime >= clock.duration) return;
  clock.playStartMs = performance.now();
}

function previewClockPause(clock: PreviewClock) {
  if (clock.playStartMs === null) return;
  clock.baseTime = previewClockNow(clock);
  clock.playStartMs = null;
}

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
  // The currently-visible slot. The other slot is the preroll.
  const [activeKey, setActiveKey] = useState<"a" | "b">("a");
  const [previewGap, setPreviewGap] = useState(false);
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
  const clockRef = useRef<PreviewClock>({
    baseTime: timelineTime,
    playStartMs: null,
    rate,
    duration: timelineDurationS,
  });
  const forceMediaSyncRef = useRef(true);
  const lastSeekRequestRef = useRef(seekRequestId);
  // Shuttle mode (>2× effective rate): element paused, frames stepped
  // off the clock. Refs because the rVFC-driven driver and media
  // event handlers both consult them without re-rendering.
  const shuttleRef = useRef(false);
  const lastShuttleStepMsRef = useRef(0);
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
  const activeMotionSceneLayers = useMemo(
    () => activeMotionSceneOverlays(timelineSnapshot, projectRoot),
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
  useEffect(() => {
    const v = slotsRef.current[activeKey].ref.current;
    if (v) updateActiveMediaSize(activeKey, v);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeKey]);

  // Reset both slots when the segments array identity changes (e.g.
  // an apply_edl landed). This must run before the resync effect
  // below; otherwise a freshly loaded active slot can be marked
  // empty after it starts playing, leaving timeline-time frozen.
  useEffect(() => {
    slotsRef.current.a.segIdx = -1;
    slotsRef.current.b.segIdx = -1;
    forceMediaSyncRef.current = true;
  }, [segments]);

  useEffect(() => {
    const clock = clockRef.current;
    clock.duration = timelineDurationS;
    if (clock.baseTime > timelineDurationS && timelineDurationS > 0) {
      clock.baseTime = timelineDurationS;
    }
  }, [timelineDurationS]);

  useEffect(() => {
    const clock = clockRef.current;
    clock.baseTime = previewClockNow(clock);
    clock.playStartMs = clock.playStartMs === null ? null : performance.now();
    clock.rate = safePlaybackRate(rate);
  }, [rate]);

  useEffect(() => {
    if (lastSeekRequestRef.current === seekRequestId) return;
    lastSeekRequestRef.current = seekRequestId;
    previewClockSeek(clockRef.current, seekTargetS);
    forceMediaSyncRef.current = true;
  }, [seekRequestId, seekTargetS]);

  useEffect(() => {
    if (isPlaying) {
      previewClockPlay(clockRef.current);
      return;
    }
    previewClockPause(clockRef.current);
    forceMediaSyncRef.current = true;
    pauseSlot(slotsRef.current.a);
    pauseSlot(slotsRef.current.b);
  }, [isPlaying]);

  useEffect(() => {
    if (!isPlaying) return;
    let frame = 0;
    const tick = () => {
      const next = previewClockNow(clockRef.current);
      setTimelineTime(next);
      if (timelineDurationS > 0 && next >= timelineDurationS) {
        setPlaying(false);
        return;
      }
      frame = window.requestAnimationFrame(tick);
    };
    frame = window.requestAnimationFrame(tick);
    return () => window.cancelAnimationFrame(frame);
  }, [isPlaying, setPlaying, setTimelineTime, timelineDurationS]);

  // The timeline clock is authoritative. This effect maps the current
  // timeline time to one media slot and only seeks the element on real
  // discontinuities: external seek, segment entry, pause, or large drift.
  useEffect(() => {
    const segIdx = findActiveSegment(segments, timelineTime);
    if (segIdx < 0) {
      const nextIdx = findNextSegmentAfter(segments, timelineTime);
      if (
        nextIdx >= 0 &&
        shouldAutoAdvanceTimelineGap(timelineTime, segments[nextIdx].timelineStart)
      ) {
        requestTimelineSeek(segments[nextIdx].timelineStart);
        return;
      }
      setPreviewGap(true);
      pauseSlot(slotsRef.current.a);
      pauseSlot(slotsRef.current.b);
      return;
    }
    setPreviewGap(false);
    const seg = segments[segIdx];
    let activeKeyNow = activeKeyRef.current;
    let active = slotsRef.current[activeKeyNow];
    const inactiveKeyNow: "a" | "b" = activeKeyNow === "a" ? "b" : "a";
    const inactive = slotsRef.current[inactiveKeyNow];
    let enteredSegment = false;

    if (active.segIdx !== segIdx) {
      if (inactive.segIdx === segIdx) {
        pauseSlot(active);
        activeKeyNow = inactiveKeyNow;
        activeKeyRef.current = activeKeyNow;
        setActiveKey(activeKeyNow);
        active = inactive;
        const preloaded = active.ref.current;
        if (preloaded) updateActiveMediaSize(activeKeyNow, preloaded);
      } else {
        ensureSlotLoaded(active, seg);
        active.segIdx = segIdx;
      }
      enteredSegment = true;
    }
    const v = active.ref.current;
    if (!v) return;
    applySegmentPlaybackSettings(
      v,
      seg,
      fadeGainMultiplier({
        timelineTimeS: timelineTime,
        startS: seg.timelineStart,
        endS: seg.timelineEnd,
        fadeInS: seg.fadeInS,
        fadeOutS: seg.fadeOutS,
      }),
    );
    const precise = forceMediaSyncRef.current || !isPlaying;
    const desiredSource = sourceTimeForTimelineTime(seg, timelineTime);
    const shuttle =
      isPlaying && isShuttleRate(safeSegmentSpeed(seg.speed) * rate);
    shuttleRef.current = shuttle;
    if (shuttle) {
      // Shuttle: continuous decode can't sustain this rate, so the
      // element stays paused (silent, like an NLE shuttle) and frames
      // are stepped off the clock. The all-keyframe proxy makes each
      // step a single-frame decode.
      pauseSlot(active);
      if (Number.isFinite(desiredSource)) {
        const nowMs = performance.now();
        const stepDue =
          precise ||
          nowMs - lastShuttleStepMsRef.current >= SHUTTLE_STEP_MS;
        if (stepDue && Math.abs((v.currentTime || 0) - desiredSource) > 0.001) {
          tryAssignCurrentTime(v, desiredSource);
          lastShuttleStepMsRef.current = nowMs;
        }
      }
      forceMediaSyncRef.current = false;
      primePreroll(segIdx);
      return;
    }
    if (Number.isFinite(desiredSource)) {
      const action = driftRecoveryAction({
        precise,
        entry: enteredSegment && !precise,
        elementPaused: v.paused,
        driftS: desiredSource - (v.currentTime || 0),
      });
      if (action === "seekElement") {
        tryAssignCurrentTime(v, desiredSource);
      } else if (action === "rebaseClock") {
        previewClockSeek(
          clockRef.current,
          timelineTimeForSegmentPosition(seg, v.currentTime || 0),
        );
      }
    }
    forceMediaSyncRef.current = false;
    if (isPlaying) {
      playSlot(active);
    } else {
      pauseSlot(active);
    }
    primePreroll(segIdx);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [segments, timelineTime, seekRequestId, seekTargetS, isPlaying]);

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
    if (shuttleRef.current) return; // shuttle owns the paused element
    playSlot(slot);
  }

  function playSlot(slot: Slot) {
    const v = slot.ref.current;
    if (!v || !v.paused) return;
    v.play().catch((err) => {
      setMediaError(`Playback failed: ${String(err)}`);
      setPlaying(false);
    });
  }

  function pauseSlot(slot: Slot) {
    const v = slot.ref.current;
    if (v && !v.paused) v.pause();
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

  function alignSlotAfterLoad(key: "a" | "b") {
    const slot = slotsRef.current[key];
    const v = slot.ref.current;
    if (!v || slot.segIdx < 0) return;
    const seg = segmentsRef.current[slot.segIdx];
    if (!seg) return;
    const isActive = key === activeKeyRef.current;
    if (isActive && useMediaStore.getState().isPlaying) {
      // Shuttle stepping completes a seek per step; the element is
      // SUPPOSED to lag the clock by up to one step — don't re-base.
      if (shuttleRef.current) return;
      // canplay/loadedmetadata after a seek or decode underrun while
      // playback runs. The wall clock kept advancing while the decoder
      // worked, so re-seeking the element against it would stall again
      // and cascade (seek → canplay → clock ran ahead → seek …). Hold
      // the video; move the CLOCK to where the media actually is.
      const mapped = timelineTimeForSegmentPosition(seg, v.currentTime);
      if (Math.abs(previewClockNow(clockRef.current) - mapped) > 0.05) {
        previewClockSeek(clockRef.current, mapped);
      }
      playActiveSlotIfNeeded(slot);
      return;
    }
    const desired = isActive
      ? sourceTimeForTimelineTime(seg, useMediaStore.getState().timelineTime)
      : seg.sourceStart;
    if (Math.abs(v.currentTime - desired) > 0.05) {
      tryAssignCurrentTime(v, desired);
    }
    playActiveSlotIfNeeded(slot);
  }

  function updateActiveMediaSize(key: "a" | "b", v: HTMLVideoElement) {
    if (key !== activeKeyRef.current) return;
    const width = v.videoWidth;
    const height = v.videoHeight;
    if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) {
      return;
    }
    setActiveMediaSize((previous) =>
      previous?.width === width && previous.height === height
        ? previous
        : { width, height },
    );
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
    const sourceTime = sourceTimeForTimelineTime(seg, timelineTime);
    const sec = Math.floor(sourceTime);
    const key = `${stem}:${sec}:${isPlaying ? "play" : "pause"}`;
    if (key === lastViewKeyRef.current) return;
    lastViewKeyRef.current = key;
    invoke("set_view_state", {
      stem,
      currentTimeS: sourceTime,
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
    // User gesture — the one place WebKit lets a suspended
    // AudioContext start.
    resumePreviewAudio();
    if (isPlaying) {
      setPlaying(false);
    } else {
      if (timelineDurationS > 0 && timelineTime >= timelineDurationS) {
        requestTimelineSeek(0);
        previewClockSeek(clockRef.current, 0);
      }
      setPlaying(true);
    }
  }

  useEffect(() => {
    const v = slotsRef.current[activeKeyRef.current].ref.current;
    if (!v) return;
    if (isPlaying && v.paused) {
      // In shuttle mode the element is intentionally paused; the
      // driver steps frames instead.
      if (shuttleRef.current) return;
      v.play().catch((err) => {
        setMediaError(`Playback failed: ${String(err)}`);
        setPlaying(false);
      });
    } else if (!isPlaying && !v.paused) {
      v.pause();
    }
  }, [activeKey, isPlaying, setMediaError, setPlaying]);

  function applySegmentPlaybackSettings(
    v: HTMLVideoElement,
    seg: PlaySegment,
    fadeMul = 1,
  ) {
    // Audio: WebAudio gain stage first — it has no 1.0 ceiling, so
    // clip volumes above unity are actually audible, and fades ride
    // the same node. Element volume pins at 1 while routed; if the
    // graph is unavailable the legacy clamped path takes over.
    const segmentVolume = Number.isFinite(seg.volume)
      ? Math.max(0, Math.min(4, seg.volume))
      : 1;
    const targetGain = segmentVolume * Math.max(0, Math.min(1, volume)) * fadeMul;
    if (setPreviewElementGain(v, targetGain)) {
      if (v.volume !== 1) v.volume = 1;
    } else {
      const clamped = Math.max(0, Math.min(1, targetGain));
      if (Math.abs(v.volume - clamped) > 0.001) v.volume = clamped;
    }
    const speed = safeSegmentSpeed(seg.speed);
    const effectiveRate = Math.max(0.0625, Math.min(16, speed * rate));
    if (Math.abs(v.playbackRate - effectiveRate) > 0.001) v.playbackRate = effectiveRate;
    // Live color preview. The WebGL grade pass (GradeCanvas) is the
    // primary path — full seven-field fidelity against the render
    // chain; while it paints, the element carries no CSS filter. The
    // CSS approximation (exposure/contrast/saturation only) remains
    // as the fallback when WebGL is unavailable or the pass is
    // suspended. Set imperatively: `filter` isn't in the React style
    // props, so the reconciler leaves it alone.
    const colorOverride = useColorPreviewOverride.getState().override;
    const colorSource =
      colorOverride && colorOverride.clipUuid === seg.clipUuid
        ? colorOverride.values
        : seg.colorCorrection;
    const colorFilter = gradePassActiveRef.current
      ? ""
      : colorPreviewCssFilter(colorSource);
    if (v.style.filter !== colorFilter) v.style.filter = colorFilter;
    // Above 2× the audio time-stretcher (pitch correction) is real
    // CPU that competes with video decode — shuttle-style playback
    // drops it, like every NLE. WebKit ships it prefixed.
    const wantPitchCorrection = effectiveRate <= 2;
    const media = v as HTMLVideoElement & {
      preservesPitch?: boolean;
      webkitPreservesPitch?: boolean;
    };
    if (typeof media.preservesPitch === "boolean") {
      if (media.preservesPitch !== wantPitchCorrection) {
        media.preservesPitch = wantPitchCorrection;
      }
    } else if (typeof media.webkitPreservesPitch === "boolean") {
      if (media.webkitPreservesPitch !== wantPitchCorrection) {
        media.webkitPreservesPitch = wantPitchCorrection;
      }
    }
  }

  // Re-apply playback settings outside the driver tick: master
  // volume/rate changes, and live color-override drags — the driver
  // only runs while timeline time moves, but color work usually
  // happens paused.
  const liveColorOverride = useColorPreviewOverride((s) => s.override);
  // WebGL grade pass: true while the canvas is painting the active
  // clip's grade — the CSS-filter approximation stands down then.
  const gradePassActiveRef = useRef(false);
  const getActiveVideo = useCallback(
    () => slotsRef.current[activeKeyRef.current].ref.current,
    [],
  );
  const activeGradeSegIdx = findActiveSegment(segments, timelineTime);
  const activeGradeSeg =
    activeGradeSegIdx >= 0 ? segments[activeGradeSegIdx] : null;
  const activeGrade = activeGradeSeg
    ? liveColorOverride && liveColorOverride.clipUuid === activeGradeSeg.clipUuid
      ? liveColorOverride.values
      : activeGradeSeg.colorCorrection
    : null;
  useEffect(() => {
    const slot = slotsRef.current[activeKeyRef.current];
    const v = slot.ref.current;
    const seg = segmentsRef.current[slot.segIdx];
    if (v && seg) applySegmentPlaybackSettings(v, seg);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeKey, volume, rate, liveColorOverride]);

  function onVideoError(e: React.SyntheticEvent<HTMLVideoElement>) {
    const el = e.currentTarget;
    const err = el.error;
    const code = err ? `code ${err.code}` : "unknown error";
    // Only the visible slot's failures warrant blanking the monitor.
    // The hidden preroll slot errors transiently while its media URL
    // is still resolving; if its media is genuinely broken, the same
    // error re-fires when it becomes the active slot.
    if (el !== slotsRef.current[activeKeyRef.current].ref.current) {
      // eslint-disable-next-line no-console
      console.warn(`preroll slot media error (${code})`);
      return;
    }
    setMediaError(`Preview media failed to load (${code}).`);
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
    e.currentTarget.setPointerCapture(e.pointerId);
    seekFromScrubPointer(e.currentTarget, e.clientX);
  }

  function onScrubPointerMove(e: React.PointerEvent<HTMLInputElement>) {
    if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
    seekFromScrubPointer(e.currentTarget, e.clientX);
  }

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

  const monitorShellRef = useRef<HTMLDivElement | null>(null);
  const stackRef = useRef<HTMLDivElement | null>(null);
  const [monitorShellSize, setMonitorShellSize] = useState<{
    width: number;
    height: number;
  }>({
    width: 0,
    height: 0,
  });
  const [stackSize, setStackSize] = useState<{ width: number; height: number }>({
    width: 0,
    height: 0,
  });
  const [activeMediaSize, setActiveMediaSize] = useState<{
    width: number;
    height: number;
  } | null>(null);
  useLayoutEffect(() => {
    const el = monitorShellRef.current;
    if (!el) return;
    const update = () => {
      setMonitorShellSize({ width: el.clientWidth, height: el.clientHeight });
    };
    update();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => update());
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
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
  const monitorFrame = useMemo(
    () =>
      containedProgramFrame(
        monitorShellSize.width,
        monitorShellSize.height,
        activeMediaSize?.width ?? 16,
        activeMediaSize?.height ?? 9,
      ),
    [activeMediaSize, monitorShellSize.height, monitorShellSize.width],
  );
  const monitorFrameCss = useMemo(
    () => programFrameStyle(monitorFrame),
    [monitorFrame],
  );
  const programFrameCss = useMemo(
    () => programFrameStyle(null),
    [],
  );
  const programFrameSize = stackSize;

  // Stage's clock shim: no ready-made PreviewClock accessor object
  // exists, so we build one that delegates to the exact same sources
  // the rest of this component already reads — timelineTime from the
  // media store, isPlaying from the media store, rate from the prop.
  // Latest values live in a ref (updated every render) so a stale
  // closure from an earlier render can never leak into Stage's frame
  // math, even though Stage itself is reconstructed on every render.
  const stageLiveValuesRef = useRef({ timelineTime, isPlaying, rate });
  stageLiveValuesRef.current = { timelineTime, isPlaying, rate };
  const stageClock = useMemo(
    () =>
      livePreviewClock({
        now: () => stageLiveValuesRef.current.timelineTime,
        isPlaying: () => stageLiveValuesRef.current.isPlaying,
        rate: () => stageLiveValuesRef.current.rate,
      }),
    [],
  );

  const monitorShellStyle = useMemo(
    () =>
      ({
        "--monitor-aspect": `${activeMediaSize?.width ?? 16} / ${activeMediaSize?.height ?? 9}`,
      }) as CSSProperties,
    [activeMediaSize],
  );

  // Publish the media's aspect on :root so ancestor chrome (the
  // program monitor box in PreviewSurface) can size itself to the
  // picture. CSS variables only flow downward, and the box can't
  // derive this from content height (WebKit intrinsic sizing zeroes
  // the inner percentage-height chain). --monitor-aspect-num is w/h
  // for `aspect-ratio`; --monitor-invaspect is h/w for height calcs.
  useEffect(() => {
    const width = activeMediaSize?.width ?? 16;
    const height = activeMediaSize?.height ?? 9;
    const root = document.documentElement;
    root.style.setProperty("--monitor-aspect-num", (width / height).toFixed(6));
    root.style.setProperty("--monitor-invaspect", (height / width).toFixed(6));
    // Mirror into the media store so React chrome (the stage context
    // bar's format badge) can read the real pixel size too.
    useMediaStore.getState().setActiveMediaSize(activeMediaSize);
    return () => {
      root.style.removeProperty("--monitor-aspect-num");
      root.style.removeProperty("--monitor-invaspect");
      useMediaStore.getState().setActiveMediaSize(null);
    };
  }, [activeMediaSize]);

  return (
    <div className="video-wrap">
      <div
        className="video-monitor-shell"
        ref={monitorShellRef}
        style={monitorShellStyle}
      >
        <div className="video-stack" ref={stackRef} style={monitorFrameCss}>
          {mediaError && <MediaErrorOverlay message={mediaError} />}
          <video
            ref={refA}
            className="video-el"
            preload="auto"
            crossOrigin="anonymous"
            style={styleA}
            onLoadedMetadata={(event) => {
              updateActiveMediaSize("a", event.currentTarget);
              alignSlotAfterLoad("a");
            }}
            onCanPlay={() => {
              setMediaError(null);
              alignSlotAfterLoad("a");
            }}
            onError={onVideoError}
            onClick={activeKey === "a" ? togglePlay : undefined}
          />
          <video
            ref={refB}
            className="video-el"
            preload="auto"
            crossOrigin="anonymous"
            style={styleB}
            onLoadedMetadata={(event) => {
              updateActiveMediaSize("b", event.currentTarget);
              alignSlotAfterLoad("b");
            }}
            onCanPlay={() => {
              setMediaError(null);
              alignSlotAfterLoad("b");
            }}
            onError={onVideoError}
            onClick={activeKey === "b" ? togglePlay : undefined}
          />
          <GradeCanvas
            grade={activeGrade}
            lutPath={activeGradeSeg?.lutPath ?? null}
            projectRoot={projectRoot}
            lutStrength={activeGradeSeg?.lutStrength ?? null}
            getVideo={getActiveVideo}
            isPlaying={isPlaying}
            suspended={activeTransition !== null}
            availabilityRef={gradePassActiveRef}
          />
          <Stage
            clock={stageClock}
            programFrameCss={programFrameCss}
            programFrameSize={programFrameSize}
            projectRoot={projectRoot}
            videoOverlays={activeVideoOverlays}
            transition={activeTransition}
            renderTransitionOnGpu={shouldRenderTransitionOnGpu(activeTransition)}
            titles={activeTitles}
            motionSceneLayers={activeMotionSceneLayers}
            broadcastOverlay={timelineSnapshot.broadcast_overlay}
            showGap={previewGap}
          />
        </div>
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
export function tryAssignCurrentTime(v: HTMLVideoElement, t: number) {
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
