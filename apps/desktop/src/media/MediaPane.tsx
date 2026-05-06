// Right-side media pane: video preview + custom transport row.
// Source is always the proxy mp4 (720p H.264, all-keyframes) — never
// the original — so scrubbing is fast on any source bitrate.
//
// Custom controls: play/pause button, scrub bar, MM:SS / MM:SS
// time display. Native HTML5 controls hidden because they overlay
// platform-styled UI (AirPlay, PiP, volume) that clashes with the
// app's flat dark theme. Volume is left to the OS for now.

import { useEffect, useRef } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { useMediaStore } from "./store";
import { useAgentStore } from "../agent/store";
import { useProjectStore } from "../app/state";
import { usePlaySegments } from "../timeline/usePlaySegments";
import { SegmentedVideoView } from "./SegmentedVideoView";

export function MediaPane() {
  const proxies = useMediaStore((s) => s.proxies);
  const selectedStem = useMediaStore((s) => s.selectedStem);
  const select = useMediaStore((s) => s.select);
  const refresh = useMediaStore((s) => s.refresh);
  const projectRoot = useProjectStore((s) => s.current);

  const items = useAgentStore((s) => s.items);

  // The OTIO timeline determines which preview shape we render:
  //   - Has clips with proxies → SegmentedVideoView (timeline output)
  //   - Empty / nothing playable → fall back to source-asset preview
  // The timeline-shaped preview behaves like a real NLE: scrub bar
  // is timeline duration, playback hops between clips at cuts. The
  // source-asset preview only shows up when the project has no
  // timeline yet (pre-import / pre-auto-insert).
  const segments = usePlaySegments();
  const showTimelinePreview = segments.length > 0;

  // Refresh proxies once at mount, and again whenever a transcode
  // job lands as Completed — that's the signal a new proxy has been
  // written to disk.
  useEffect(() => {
    refresh();
  }, [projectRoot, refresh]);

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
  const src = selected ? convertFileSrc(selected.proxy_path) : null;

  return (
    <aside className="media-pane">
      <header className="media-header">
        <span className="media-label">
          {showTimelinePreview ? "Preview · timeline" : "Preview"}
        </span>
        {/* Asset dropdown only appears when we're in source-preview
            mode. Once the timeline has clips, the preview IS the
            timeline output — there is no per-asset choice to make,
            same as Resolve / Premiere / Final Cut. */}
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

function VideoView({ src, stem }: { src: string; stem: string }) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const setTime = useMediaStore((s) => s.setTime);
  const setDuration = useMediaStore((s) => s.setDuration);
  const setPlaying = useMediaStore((s) => s.setPlaying);
  const currentTime = useMediaStore((s) => s.currentTime);
  const durationS = useMediaStore((s) => s.durationS);
  const isPlaying = useMediaStore((s) => s.isPlaying);
  const seekRequestId = useMediaStore((s) => s.seekRequestId);
  const seekTargetS = useMediaStore((s) => s.seekTargetS);
  const lastPushedViewRef = useRef<string>("");

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
      v.play().catch(() => {});
    } else {
      v.pause();
    }
  }

  function onScrub(e: React.ChangeEvent<HTMLInputElement>) {
    const v = videoRef.current;
    const t = Number(e.target.value);
    if (v && Number.isFinite(t)) {
      v.currentTime = t;
      setTime(t);
    }
  }

  return (
    <div className="video-wrap">
      <video
        ref={videoRef}
        className="video-el"
        preload="metadata"
        src={src}
        onTimeUpdate={(e) => setTime(e.currentTarget.currentTime)}
        onLoadedMetadata={(e) => setDuration(e.currentTarget.duration)}
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
          max={durationS || 0}
          step={0.01}
          value={currentTime}
          onChange={onScrub}
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
