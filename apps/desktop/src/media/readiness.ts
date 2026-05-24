import type {
  MediaCacheArtifactStatus,
  MediaFailureReason,
  MediaReadinessEntry,
  MediaReadinessSnapshot,
  PlayableArtifact,
} from "../protocol";
import type { SourceMediaEntry } from "./store";
import type { MediaIndexingStatus } from "../ui";

export type MediaReadinessUi = {
  status: MediaIndexingStatus;
  detailSuffix: string;
  progress?: number;
};

export function findMediaReadinessEntry(
  snapshot: MediaReadinessSnapshot | undefined,
  source: SourceMediaEntry,
): MediaReadinessEntry | undefined {
  if (!snapshot) return undefined;
  const sourceBaseName = baseName(source.path);
  return snapshot.entries.find(
    (entry) =>
      entry.asset_id === source.id ||
      entry.display_name === source.name ||
      entry.display_name === sourceBaseName ||
      entry.source_path === source.path,
  );
}

export function mediaReadinessUi(entry: MediaReadinessEntry): MediaReadinessUi {
  switch (entry.state) {
    case "ready":
      return {
        status: "indexed",
        detailSuffix: playableLabel(entry.playable) ?? "playback ready",
      };
    case "partial":
      return {
        status: "partial",
        detailSuffix: partialLabel(entry),
        progress: entry.transcript_progress?.percent ?? undefined,
      };
    case "processing":
      return {
        status: "processing",
        detailSuffix: processingLabel(entry),
        progress: entry.transcript_progress?.percent ?? undefined,
      };
    case "blocked":
      return {
        status: "failed",
        detailSuffix: `blocked: ${failureSummary(entry)}`,
      };
    case "failed":
      return {
        status: "failed",
        detailSuffix: failureSummary(entry),
      };
    case "offline":
      return {
        status: "missing",
        detailSuffix: failureSummary(entry),
      };
  }
}

function partialLabel(entry: MediaReadinessEntry): string {
  const playable = playableLabel(entry.playable);
  if (entry.transcript_progress) {
    return playable
      ? `${playable} · ${entry.transcript_progress.label}`
      : entry.transcript_progress.label;
  }
  const pending = pendingCacheLabels(entry).slice(0, 2);
  if (playable && pending.length > 0) {
    return `${playable} · waiting for ${pending.join(", ")}`;
  }
  if (playable) return `${playable} · indexes pending`;
  const ready = readyCacheLabels(entry).slice(0, 2);
  if (ready.length > 0) return `${ready.join(", ")} ready`;
  return "partially ready";
}

function processingLabel(entry: MediaReadinessEntry): string {
  if (entry.transcript_progress) return entry.transcript_progress.label;
  const pending = pendingCacheLabels(entry).slice(0, 2);
  if (pending.length > 0) return `building ${pending.join(", ")}`;
  return "processing media";
}

function failureSummary(entry: MediaReadinessEntry): string {
  if (entry.failures.length > 0) {
    return entry.failures.map(failureLabel).join(", ");
  }
  const failed = failedCacheLabels(entry).slice(0, 2);
  if (failed.length > 0) return `${failed.join(", ")} failed`;
  return "media unavailable";
}

function playableLabel(playable: PlayableArtifact | null): string | null {
  if (!playable) return null;
  switch (playable.kind) {
    case "source":
      return playable.backend === "webkit"
        ? "source playable"
        : `source playable via ${backendLabel(playable.backend)}`;
    case "proxy":
      return "proxy ready";
    case "compatibility_media":
      return "compatibility media ready";
    case "stream":
      return "stream ready";
  }
}

function pendingCacheLabels(entry: MediaReadinessEntry): string[] {
  return cacheLabels(entry, "pending");
}

function readyCacheLabels(entry: MediaReadinessEntry): string[] {
  return cacheLabels(entry, "ready");
}

function failedCacheLabels(entry: MediaReadinessEntry): string[] {
  return cacheLabels(entry, "failed");
}

function cacheLabels(
  entry: MediaReadinessEntry,
  status: MediaCacheArtifactStatus,
): string[] {
  return [
    cacheLabel("proxy", entry.cache.proxy, status),
    cacheLabel("compatibility media", entry.cache.compatibility_media, status),
    cacheLabel("thumbnails", entry.cache.thumbnails, status),
    cacheLabel("waveform", entry.cache.waveform, status),
    cacheLabel("transcript", entry.cache.transcript, status),
    cacheLabel("captions", entry.cache.captions, status),
    cacheLabel("scenes", entry.cache.scenes, status),
    cacheLabel("face detection", entry.cache.face_detection, status),
    cacheLabel("color analysis", entry.cache.color_analysis, status),
    cacheLabel("motion analysis", entry.cache.motion_analysis, status),
    cacheLabel("audio analysis", entry.cache.audio_analysis, status),
    cacheLabel("silence detection", entry.cache.silence_detection, status),
  ].filter((label): label is string => Boolean(label));
}

function cacheLabel(
  label: string,
  actual: MediaCacheArtifactStatus,
  expected: MediaCacheArtifactStatus,
): string | null {
  return actual === expected ? label : null;
}

function failureLabel(reason: MediaFailureReason): string {
  switch (reason) {
    case "source_missing":
      return "source file missing";
    case "source_outside_project":
      return "source outside project";
    case "unsupported_container":
      return "unsupported container";
    case "unsupported_codec":
      return "unsupported codec";
    case "browser_decode_failed":
      return "browser cannot decode source";
    case "proxy_missing":
      return "proxy missing";
    case "proxy_failed":
      return "proxy generation failed";
    case "compatibility_media_missing":
      return "compatibility media missing";
    case "compatibility_media_failed":
      return "compatibility media failed";
    case "cache_missing":
      return "cache missing";
    case "cache_failed":
      return "cache failed";
    case "disk_space_low":
      return "not enough disk space";
    case "ffmpeg_unavailable":
      return "FFmpeg unavailable";
    case "probe_failed":
      return "media probe failed";
    case "unknown":
      return "unknown media failure";
  }
}

function backendLabel(backend: PlayableArtifact["backend"]): string {
  switch (backend) {
    case "webkit":
      return "WebKit";
    case "ffmpeg_remux":
      return "FFmpeg remux";
    case "ffmpeg_transcode":
      return "FFmpeg transcode";
    case "libmpv":
      return "libmpv";
    case "native":
      return "native decoder";
    case "unknown":
      return "unknown decoder";
  }
}

function baseName(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  return normalized.split("/").pop() ?? normalized;
}
