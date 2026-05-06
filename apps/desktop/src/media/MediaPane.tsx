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

import { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { useMediaStore } from "./store";
import { useAgentStore } from "../agent/store";
import { useProjectStore } from "../app/state";
import { useTimelineStore } from "../timeline/store";
import { usePlaySegments } from "../timeline/usePlaySegments";
import { findActiveSegment } from "../timeline/usePlaySegments";
import { SegmentedVideoView } from "./SegmentedVideoView";
import { TranscriptView } from "../transcript/TranscriptView";
import { useTranscriptStore } from "../transcript/store";

type Tab = "preview" | "transcript";

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
  const showTimelinePreview = timelineDurationS > 0;

  // The "active stem" — what the transcript pane is keyed against.
  // In timeline mode it's the proxy stem of the segment under the
  // playhead; in source-preview mode it's the user-selected asset.
  // Keeping this derivation here (not in the transcript store)
  // means transcript-pane state stays a pure cache; the view
  // determines what's active.
  const segments = usePlaySegments();
  const timelineTime = useMediaStore((s) => s.timelineTime);
  const activeStem = useMemo(() => {
    if (showTimelinePreview) {
      const idx = findActiveSegment(segments, timelineTime);
      if (idx >= 0) return stemFromProxyPath(segments[idx].proxyPath);
      // Past end of timeline → use the last segment's stem so the
      // transcript pane doesn't blank.
      if (segments.length > 0) {
        return stemFromProxyPath(segments[segments.length - 1].proxyPath);
      }
      return null;
    }
    return selectedStem;
  }, [showTimelinePreview, segments, timelineTime, selectedStem]);

  // Transcript fetch is keyed off activeStem. The store dedupes
  // in-flight loads so this fires once per stem change.
  const setActiveStem = useTranscriptStore((s) => s.setActiveStem);
  const transcriptState = useTranscriptStore((s) =>
    activeStem ? s.byStem[activeStem] : undefined,
  );
  const clearTranscriptCache = useTranscriptStore((s) => s.clearCache);
  useEffect(() => {
    setActiveStem(activeStem);
  }, [activeStem, setActiveStem]);

  // Tab state. Default to whichever tab has content: transcript if
  // the active stem has a sidecar; preview otherwise. The user can
  // override by clicking the other tab — once they switch, we
  // remember their choice for the rest of this session (don't
  // clobber on every active-stem change).
  const [tabExplicit, setTabExplicit] = useState<Tab | null>(null);
  const autoTab: Tab =
    transcriptState && transcriptState.state === "loaded"
      ? "transcript"
      : "preview";
  const tab = tabExplicit ?? autoTab;

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
      // Re-trigger the active-stem load after clearing.
      if (activeStem) setActiveStem(activeStem);
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
  const src = selected ? convertFileSrc(selected.proxy_path) : null;

  const transcriptAvailable =
    transcriptState !== undefined && transcriptState.state === "loaded";

  return (
    <aside className="media-pane">
      <header className="media-header">
        <div className="media-tabs" role="tablist">
          <button
            role="tab"
            aria-selected={tab === "preview"}
            className={`media-tab${tab === "preview" ? " media-tab-active" : ""}`}
            onClick={() => setTabExplicit("preview")}
          >
            {showTimelinePreview ? "Preview · timeline" : "Preview"}
          </button>
          <button
            role="tab"
            aria-selected={tab === "transcript"}
            className={`media-tab${tab === "transcript" ? " media-tab-active" : ""}`}
            onClick={() => setTabExplicit("transcript")}
            disabled={!activeStem}
            title={
              transcriptAvailable
                ? undefined
                : "Run whisper indexing on this asset to enable the transcript tab"
            }
          >
            Transcript
            {!transcriptAvailable && activeStem ? " ·" : ""}
            {transcriptState?.state === "loading"
              ? <span className="media-tab-meta"> loading…</span>
              : transcriptState?.state === "missing"
              ? <span className="media-tab-meta"> not indexed</span>
              : null}
          </button>
        </div>
        {/* Asset dropdown only appears in source-preview mode AND on
            the Preview tab. Once the timeline has clips, the preview
            IS the timeline output — there is no per-asset choice to
            make. */}
        {tab === "preview" && !showTimelinePreview && proxies.length > 1 && (
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
        {tab === "transcript" ? (
          <TranscriptView stem={activeStem} />
        ) : showTimelinePreview ? (
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

function stemFromProxyPath(path: string): string | null {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const file = slash >= 0 ? path.slice(slash + 1) : path;
  const dot = file.lastIndexOf(".");
  return dot > 0 ? file.slice(0, dot) : file || null;
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
