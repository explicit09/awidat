// Right-side media pane: video preview + transport. Source is
// always the proxy mp4 (720p H.264, all-keyframes) — never the
// original — so scrubbing is fast on any source bitrate.

import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useMediaStore } from "./store";
import { useAgentStore } from "../agent/store";

export function MediaPane() {
  const proxies = useMediaStore((s) => s.proxies);
  const selectedStem = useMediaStore((s) => s.selectedStem);
  const select = useMediaStore((s) => s.select);
  const refresh = useMediaStore((s) => s.refresh);

  const items = useAgentStore((s) => s.items);
  const videoRef = useRef<HTMLVideoElement | null>(null);

  // Refresh proxies once at mount, and again whenever a transcode
  // job lands as Completed — that's the signal a new proxy has been
  // written to disk.
  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    const lastTranscodeCompleted = items
      .filter(
        (it): it is Extract<typeof items[number], { kind: "job" }> =>
          it.kind === "job" &&
          it.job_kind === "transcode" &&
          it.phase === "completed",
      )
      .map((it) => it.id);
    if (lastTranscodeCompleted.length > 0) {
      refresh();
    }
    // We re-run when the count of completed transcodes changes, not
    // on every chat-item churn, so we don't thrash the backend.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    items.filter((it) => it.kind === "job" && it.job_kind === "transcode" && it.phase === "completed").length,
  ]);

  const selected = proxies.find((p) => p.stem === selectedStem) ?? null;
  const src = selected ? convertFileSrc(selected.proxy_path) : null;

  // Reset playback when switching assets so the new video starts
  // from frame 0 instead of inheriting the prior video's currentTime.
  useEffect(() => {
    if (videoRef.current) {
      videoRef.current.currentTime = 0;
      videoRef.current.pause();
    }
  }, [src]);

  return (
    <aside className="media-pane">
      <header className="media-header">
        <span className="media-label">Preview</span>
        {proxies.length > 1 && (
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
        {src ? (
          <VideoView src={src} videoRef={videoRef} stem={selectedStem ?? ""} />
        ) : (
          <MediaEmpty hasAnyProxies={proxies.length > 0} />
        )}
      </div>
    </aside>
  );
}

function VideoView({
  src,
  videoRef,
  stem,
}: {
  src: string;
  videoRef: React.MutableRefObject<HTMLVideoElement | null>;
  stem: string;
}) {
  const [error, setError] = useState<string | null>(null);

  return (
    <div className="video-wrap">
      <video
        ref={videoRef}
        className="video-el"
        controls
        preload="metadata"
        src={src}
        onError={(e) => {
          const target = e.currentTarget;
          // Browsers don't expose much error detail; surface what
          // we can so a missing proxy or codec issue isn't silent.
          const code = target.error?.code ?? null;
          setError(`failed to load proxy (code ${code})`);
        }}
        onLoadedMetadata={() => setError(null)}
      />
      <div className="video-meta">
        <span className="video-meta-label">proxy</span>
        <code className="video-stem">{stem}</code>
      </div>
      {error && <div className="video-error">{error}</div>}
    </div>
  );
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
