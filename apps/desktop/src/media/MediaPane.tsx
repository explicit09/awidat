// Right-side media pane: tabbed Preview / Transcript.
//
// Preview tab (default):
//   - Timeline mode → SegmentedVideoView (plays the OTIO output)
//   - Source-preview mode → VideoView on the selected proxy
//
// Transcript tab (Step 6):
//   - Whisper transcript for the active asset, click-word-to-seek,
//     drag-select-to-delete. Auto-selected when a sidecar exists
//     for the active stem and the user hasn't manually picked
//     Preview this session.
//
// Source for the proxy mp4 is always the 720p all-keyframes file —
// never the original — so scrubbing stays fast on any source
// bitrate. Custom controls: play/pause, scrub bar, MM:SS time.
// Native HTML5 controls hidden because they overlay platform-styled
// UI (AirPlay, PiP, volume) that clashes with the dark theme.

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useMediaStore } from "./store";
import { cachedMediaStreamUrl, mediaStreamUrl } from "./mediaStreamUrl";
import { useAgentStore } from "../agent/store";
import { useProjectStore } from "../app/state";
import { useTimelineStore } from "../timeline/store";
import { SegmentedVideoView } from "./SegmentedVideoView";
import { useTranscriptStore } from "../transcript/store";

export function MediaPane() {
  const proxies = useMediaStore((s) => s.proxies);
  const selectedStem = useMediaStore((s) => s.selectedStem);
  const select = useMediaStore((s) => s.select);
  const refresh = useMediaStore((s) => s.refresh);
  const projectRoot = useProjectStore((s) => s.current);

  const items = useAgentStore((s) => s.items);

  // The OTIO timeline determines which preview shape we render:
  //   - Has clips at all → SegmentedVideoView (even if some are
  //     still transcoding — the view shows a "+ N transcoding…"
  //     hint and plays whatever segments are ready)
  //   - Empty timeline → source-asset preview (pre-import / pre-
  //     auto-insert window where the user is inspecting raw assets)
  const timelineDurationS = useTimelineStore((s) => s.snapshot.duration_s);
  const previewLimitations = useTimelineStore((s) => s.snapshot.preview_limitations);
  const showTimelinePreview = timelineDurationS > 0;

  const clearTranscriptCache = useTranscriptStore((s) => s.clearCache);

  // Refresh proxies once at mount, and again whenever a transcode
  // job lands as Completed — that's the signal a new proxy has been
  // written to disk.
  useEffect(() => {
    refresh();
  }, [projectRoot, refresh]);

  // Whisper-job Completed → invalidate transcript cache (the
  // backend already cleared its parse cache; the frontend store
  // needs to drop too so the next setActiveStem triggers a refetch).
  useEffect(() => {
    const completedWhisper = items.filter(
      (it) =>
        it.kind === "job" &&
        it.job_kind === "indexing" &&
        it.phase === "completed",
    ).length;
    if (completedWhisper > 0) {
      clearTranscriptCache();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    items.filter(
      (it) =>
        it.kind === "job" &&
        it.job_kind === "indexing" &&
        it.phase === "completed",
    ).length,
  ]);

  useEffect(() => {
    const completedTranscodes = items.filter(
      (it) =>
        it.kind === "job" &&
        it.job_kind === "transcode" &&
        it.phase === "completed",
    ).length;
    if (completedTranscodes > 0) {
      refresh();
    }
    // We re-run when the count of completed transcodes changes, not
    // on every chat-item churn, so we don't thrash the backend.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    items.filter(
      (it) =>
        it.kind === "job" &&
        it.job_kind === "transcode" &&
        it.phase === "completed",
    ).length,
  ]);

  const selected = proxies.find((p) => p.stem === selectedStem) ?? null;

  const [src, setSrc] = useState<string | null>(() =>
    selected ? cachedMediaStreamUrl(selected.proxy_path) ?? null : null,
  );
  useEffect(() => {
    if (!selected) {
      setSrc(null);
      return;
    }
    const cached = cachedMediaStreamUrl(selected.proxy_path);
    if (cached) {
      setSrc(cached);
      return;
    }
    let cancelled = false;
    mediaStreamUrl(selected.proxy_path)
      .then((url) => {
        if (!cancelled) setSrc(url);
      })
      .catch((e) => console.warn("media stream url failed", e));
    return () => {
      cancelled = true;
    };
  }, [selected?.proxy_path]);

  return (
    <aside className="media-pane">
      <header className="media-header">
        <div className="media-titleblock">
          <span className="media-kicker">Viewer</span>
          <strong>{showTimelinePreview ? "Timeline preview" : "Source preview"}</strong>
        </div>
        <div className="media-status-strip">
          <span>{showTimelinePreview ? "Cut output" : "Proxy"}</span>
          {showTimelinePreview && <code>{formatTime(timelineDurationS)}</code>}
        </div>
        {/* Asset dropdown only appears in source-preview mode AND on
            the Preview tab. Once the timeline has clips, the preview
            IS the timeline output — there is no per-asset choice to
            make. */}
        {!showTimelinePreview && proxies.length > 1 && (
          <select
            className="media-asset-select"
            value={selectedStem ?? ""}
            onChange={(e) => select(e.target.value || null)}
          >
            {proxies.map((p) => (
              <option key={p.stem} value={p.stem}>
                {p.stem}
              </option>
            ))}
          </select>
        )}
      </header>
      <div className="media-stage">
        {showTimelinePreview && previewLimitations.length > 0 && (
          <PreviewLimitationsBanner limitations={previewLimitations} />
        )}
        {showTimelinePreview ? (
          <SegmentedVideoView />
        ) : src ? (
          <VideoView src={src} stem={selectedStem ?? ""} />
        ) : (
          <MediaEmpty hasAnyProxies={proxies.length > 0} />
        )}
      </div>
    </aside>
  );
}

function PreviewLimitationsBanner({
  limitations,
}: {
  limitations: { kind: string; message: string }[];
}) {
  return (
    <div className="media-preview-limits" role="status" aria-live="polite">
      <span className="media-preview-limits-label">Preview caveat</span>
      <span>{limitations.map((limitation) => limitation.message).join(" ")}</span>
    </div>
  );
}

function VideoView({ src, stem }: { src: string; stem: string }) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const setTime = useMediaStore((s) => s.setTime);
  const setDuration = useMediaStore((s) => s.setDuration);
  const setPlaying = useMediaStore((s) => s.setPlaying);
  const mediaError = useMediaStore((s) => s.mediaError);
  const setMediaError = useMediaStore((s) => s.setMediaError);
  const currentTime = useMediaStore((s) => s.currentTime);
  const durationS = useMediaStore((s) => s.durationS);
  const isPlaying = useMediaStore((s) => s.isPlaying);
  const seekRequestId = useMediaStore((s) => s.seekRequestId);
  const seekTargetS = useMediaStore((s) => s.seekTargetS);
  const lastPushedViewRef = useRef<string>("");
  const scrubInputRef = useRef<HTMLInputElement | null>(null);
  const scrubPointerIdRef = useRef<number | null>(null);

  // External seek requests (from timeline canvas click/drag) drive
  // the video element imperatively. We watch seekRequestId rather
  // than seekTargetS so back-to-back seeks to the same time still
  // re-trigger.
  useEffect(() => {
    const v = videoRef.current;
    if (v && seekRequestId > 0 && Number.isFinite(seekTargetS)) {
      v.currentTime = seekTargetS;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [seekRequestId]);

  // Reset playback when switching assets so the new video starts
  // from frame 0 instead of inheriting the prior video's currentTime.
  useEffect(() => {
    const v = videoRef.current;
    if (v) {
      v.currentTime = 0;
      v.pause();
    }
    lastPushedViewRef.current = "";
  }, [src]);

  // Push view-state to the backend whenever stem / play state /
  // integer-second of currentTime changes. Throttling on integer
  // seconds keeps the IPC traffic to ~1 Hz during playback while
  // still capturing every scrub-stop and play/pause toggle.
  useEffect(() => {
    if (!stem) return;
    const sec = Math.floor(currentTime);
    const key = `${stem}:${sec}:${isPlaying ? "play" : "pause"}`;
    if (key === lastPushedViewRef.current) return;
    lastPushedViewRef.current = key;
    invoke("set_view_state", {
      stem,
      currentTimeS: currentTime,
      isPlaying,
    }).catch(() => {});
  }, [stem, currentTime, isPlaying]);

  // Keyboard: spacebar play/pause when the pane is focused.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      // Don't capture when the user is typing in the composer.
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
      v.play().catch((err) => {
        setMediaError(`Playback failed: ${String(err)}`);
      });
    } else {
      v.pause();
    }
  }

  function onVideoError(e: React.SyntheticEvent<HTMLVideoElement>) {
    const err = e.currentTarget.error;
    const code = err ? `code ${err.code}` : "unknown error";
    setMediaError(`Source preview failed to load (${code}).`);
  }

  function onScrub(e: React.ChangeEvent<HTMLInputElement>) {
    const v = videoRef.current;
    const t = Number(e.target.value);
    if (v && Number.isFinite(t)) {
      v.currentTime = t;
      setTime(t);
    }
  }

  function onScrubInput(e: React.FormEvent<HTMLInputElement>) {
    const v = videoRef.current;
    const t = Number(e.currentTarget.value);
    if (v && Number.isFinite(t)) {
      v.currentTime = t;
      setTime(t);
    }
  }

  function seekFromScrubPointer(
    el: HTMLInputElement,
    clientX: number,
  ) {
    const v = videoRef.current;
    if (!v || durationS <= 0) return;
    const rect = el.getBoundingClientRect();
    const ratio =
      rect.width > 0
        ? Math.max(0, Math.min(1, (clientX - rect.left) / rect.width))
        : 0;
    const t = ratio * durationS;
    v.currentTime = t;
    setTime(t);
  }

  function onScrubPointerDown(e: React.PointerEvent<HTMLInputElement>) {
    if (durationS <= 0) return;
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

  return (
    <div className="video-wrap">
      <div className="video-stack">
        {mediaError && <MediaErrorOverlay message={mediaError} />}
        <video
          ref={videoRef}
          className="video-el"
          preload="metadata"
          src={src}
          onTimeUpdate={(e) => setTime(e.currentTarget.currentTime)}
          onLoadedMetadata={(e) => {
            setMediaError(null);
            setDuration(e.currentTarget.duration);
          }}
          onCanPlay={() => setMediaError(null)}
          onError={onVideoError}
          onPlay={() => setPlaying(true)}
          onPause={() => setPlaying(false)}
          onEnded={() => setPlaying(false)}
          onClick={togglePlay}
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
          max={durationS || 0}
          step={0.01}
          value={currentTime}
          onChange={onScrub}
          onInput={onScrubInput}
          onPointerDown={onScrubPointerDown}
          onPointerMove={onScrubPointerMove}
          onPointerUp={(e) => finishScrubPointer(e.pointerId)}
          onPointerCancel={(e) => finishScrubPointer(e.pointerId)}
          disabled={durationS === 0}
        />
        <div className="transport-time">
          <span>{formatTime(currentTime)}</span>
          <span className="transport-time-sep">/</span>
          <span className="transport-time-total">{formatTime(durationS)}</span>
        </div>
      </div>
      <div className="video-meta">
        <span className="video-meta-label">proxy</span>
        <code className="video-stem">{stem}</code>
      </div>
    </div>
  );
}

function MediaErrorOverlay({ message }: { message: string }) {
  return (
    <div className="media-error-overlay">
      <p className="media-error-title">Preview unavailable</p>
      <p className="media-error-message">{message}</p>
    </div>
  );
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

function MediaEmpty({ hasAnyProxies }: { hasAnyProxies: boolean }) {
  return (
    <div className="media-empty">
      {hasAnyProxies ? (
        <p>Pick an asset above to preview.</p>
      ) : (
        <>
          <p className="media-empty-title">No previewable media yet.</p>
          <p className="media-empty-hint">
            Use <strong>Import file…</strong> or <strong>Import URL…</strong>{" "}
            in the bar above. The 720p proxy is generated automatically and
            shows up here when ready.
          </p>
        </>
      )}
    </div>
  );
}
