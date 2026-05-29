// Render queue worker hook.
//
// Drains the user-side `useRenderQueueStore` queue one entry at a
// time. Calls the appropriate Tauri command per kind:
//
//   * video_master  → start_timeline_render + poll_timeline_render
//   * video_reframe → start_reframe_render (progress via Job events;
//                      this worker resolves on the Job's Completed
//                      lifecycle by reading useAgentStore)
//   * captions      → export_caption_sidecars (synchronous; one shot)
//   * still         → export_still (synchronous; one shot)
//
// Sequential — never starts a second entry while another is running.
// Video reframes consume the most-recent master mp4 path; if a
// reframe is queued without a master ahead of it the worker fails
// the entry with a "no master available" error.

import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  renderQueueSelectors,
  useRenderQueueStore,
  type RenderQueueEntry,
  type RenderUploadState,
} from "./renderQueue";
import {
  useUploadMetadata,
  type UploadMetadata,
} from "../state/uploadMetadata";

type TimelineRenderInfo = {
  job_id: string;
  output_path: string;
  total_duration_s: number | null;
  render_limitations: unknown;
};

type ReframeJobInfo = {
  job_id: string;
  output_path: string;
  width: number;
  height: number;
};

type JobStatus = {
  state: "queued" | "running" | "done" | "failed" | "cancelled";
  progress_pct: number | null;
  time_done_s: number | null;
  eta_s: number | null;
  log_excerpt: string | null;
  exit_code: number | null;
};

type CaptionSidecarPaths = {
  srt_path: string;
  vtt_path: string;
  cue_count: number;
};

type StillExportInfo = {
  output_path: string;
  format: string;
};

const POLL_INTERVAL_MS = 500;

/** Hook that runs the queue. Mount it once at the app root (App.tsx);
 *  it idles when there's nothing pending. */
export function useRenderQueueWorker(): void {
  const busyRef = useRef(false);
  const lastMasterPathRef = useRef<string | null>(null);

  useEffect(() => {
    const unsub = useRenderQueueStore.subscribe((state) => {
      if (busyRef.current) return;
      const pending = renderQueueSelectors.pending(state);
      if (pending.length === 0) return;
      const next = pending[0];
      busyRef.current = true;
      void runEntry(next, lastMasterPathRef)
        .catch((err) => {
          const message = err instanceof Error ? err.message : String(err);
          useRenderQueueStore.getState().markFailed(next.id, message);
        })
        .finally(() => {
          busyRef.current = false;
        });
    });
    // Kick the worker once on mount in case there are pending entries
    // already (e.g. after page reload restoring localStorage).
    const state = useRenderQueueStore.getState();
    const pending = renderQueueSelectors.pending(state);
    if (pending.length > 0 && !busyRef.current) {
      const next = pending[0];
      busyRef.current = true;
      void runEntry(next, lastMasterPathRef)
        .catch((err) => {
          const message = err instanceof Error ? err.message : String(err);
          useRenderQueueStore.getState().markFailed(next.id, message);
        })
        .finally(() => {
          busyRef.current = false;
        });
    }
    return () => unsub();
  }, []);
}

async function runEntry(
  entry: RenderQueueEntry,
  lastMasterPathRef: React.MutableRefObject<string | null>,
): Promise<void> {
  const store = useRenderQueueStore.getState();
  if (entry.kind === "video_master") {
    store.markRunning(entry.id);
    const info = await invoke<TimelineRenderInfo>("start_timeline_render");
    store.markRunning(entry.id, info.job_id);
    lastMasterPathRef.current = info.output_path;
    await pollVideoJob(info.job_id, entry.id, info.output_path);
    await maybeChainUploads(entry, info.job_id, info.output_path);
    return;
  }
  if (entry.kind === "video_reframe") {
    const master =
      entry.reframeMasterPath ?? lastMasterPathRef.current ?? null;
    if (!master) {
      store.markFailed(
        entry.id,
        "no master render available — queue a video_master first",
      );
      return;
    }
    if (
      entry.reframeWidth === undefined ||
      entry.reframeHeight === undefined
    ) {
      store.markFailed(entry.id, "reframe target missing width/height");
      return;
    }
    store.markRunning(entry.id);
    const info = await invoke<ReframeJobInfo>("start_reframe_render", {
      masterPath: master,
      targetId: entry.targetId,
      width: entry.reframeWidth,
      height: entry.reframeHeight,
      videoBitrateKbps: entry.reframeBitrateKbps,
    });
    store.markRunning(entry.id, info.job_id);
    // Reframes emit Job items via the agent store; for now we just
    // poll a small timer to update progress from the most recent
    // emitted Item::Job for this job_id. Simpler integration than
    // hooking the agent-store subscription here.
    await pollReframeJobViaAgentStore(info.job_id, entry.id, info.output_path);
    await maybeChainUploads(entry, info.job_id, info.output_path);
    return;
  }
  if (entry.kind === "captions") {
    store.markRunning(entry.id);
    const info = await invoke<CaptionSidecarPaths>("export_caption_sidecars");
    store.markDone(entry.id, info.srt_path);
    return;
  }
  if (entry.kind === "still") {
    if (!entry.stillAssetPath || entry.stillTimecodeS === undefined) {
      store.markFailed(
        entry.id,
        "still export missing asset_path or timecode",
      );
      return;
    }
    store.markRunning(entry.id);
    const info = await invoke<StillExportInfo>("export_still", {
      assetPath: entry.stillAssetPath,
      tS: entry.stillTimecodeS,
      kind: entry.stillKind ?? "custom",
    });
    store.markDone(entry.id, info.output_path);
    return;
  }
}

/** Per-target upload entry returned by the backend. Matches Rust's
 *  `UploadJobEntry`. */
type UploadJobEntryWire = {
  job_id: string;
  upload_targets: string[];
  upload_states: Record<string, RenderUploadState>;
  published_urls: Record<string, string>;
};

/**
 * After a render lands at `done`, fan out uploads to the user's
 * selected targets. Two-step protocol:
 *
 *   1. `set_render_upload_targets(jobId, providers)` — backend
 *      registers per-target tracking.
 *   2. `start_uploads_for_job(jobId, file, title)` — backend spawns
 *      one tokio task per target, each driving the
 *      `Pending → Uploading → Published / Failed` lifecycle.
 *
 * Then this function polls `poll_upload_states` until every target
 * lands in a terminal state.
 *
 * No-op when the entry has no `uploadTargets` — the queue still
 * publishes the render-done state via `markDone` upstream, just
 * without the auto-upload chain.
 */
async function maybeChainUploads(
  entry: RenderQueueEntry,
  jobId: string,
  outputPath: string,
): Promise<void> {
  const targets = entry.uploadTargets ?? [];
  if (targets.length === 0) return;
  const store = useRenderQueueStore.getState();
  // Snapshot the per-target metadata the user typed into the Deliver
  // form. Keyed by `(entry.id, provider)` while the form is open, but
  // the backend keys by `(jobId, provider)` — so we re-key here.
  const metadataStore = useUploadMetadata.getState();
  const metadataByProvider: Record<string, UploadMetadata> = {};
  for (const provider of targets) {
    metadataByProvider[provider] = metadataStore.get(
      entry.id,
      provider,
      entry.label,
    );
  }
  try {
    await invoke<void>("set_render_upload_targets", {
      jobId,
      providers: targets,
    });
    // Push each target's metadata to the backend before we kick the
    // upload — `start_uploads_for_job` reads from there to build the
    // per-provider `UploadParams`. Awaited in parallel to keep total
    // latency to one round trip even with three targets.
    await Promise.all(
      targets.map((provider) =>
        invoke<void>("set_upload_metadata", {
          jobId,
          provider,
          metadata: metadataByProvider[provider],
        }),
      ),
    );
    // Mirror the saved metadata onto the queue entry so the retry
    // path doesn't have to re-read the form (which may have moved on
    // to a different render's metadata by then).
    store.setUploadMetadata(entry.id, metadataByProvider);
    await invoke<void>("start_uploads_for_job", {
      jobId,
      filePath: outputPath,
      title: entry.label,
    });
  } catch (err) {
    // Couldn't register / kick — surface as failed states for every
    // target so the UI doesn't sit on "Pending" forever.
    const message = err instanceof Error ? err.message : String(err);
    const states: Record<string, RenderUploadState> = Object.fromEntries(
      targets.map((p) => [p, { state: "failed", reason: message }]),
    );
    store.setUploadStates(entry.id, states, {});
    return;
  }
  await pollUploadStates(entry.id, jobId);
}

const UPLOAD_POLL_INTERVAL_MS = 800;

/** Poll backend `poll_upload_states` until every target terminal. */
async function pollUploadStates(queueId: string, jobId: string): Promise<void> {
  const store = useRenderQueueStore.getState();
  // Safety bound: ~10 minutes at 800ms cadence. Long-running uploads
  // (multi-GB to YouTube) can outrun this; that's fine — the worker
  // returns, the UI reflects the last-seen states, and the user can
  // hit "Retry" to re-poll on demand.
  const maxTicks = 750;
  for (let tick = 0; tick < maxTicks; tick++) {
    await sleep(UPLOAD_POLL_INTERVAL_MS);
    let snapshot: UploadJobEntryWire | null;
    try {
      snapshot = await invoke<UploadJobEntryWire | null>(
        "poll_upload_states",
        { jobId },
      );
    } catch {
      // Treat IPC errors as "try again next tick" — the backend may
      // not be ready yet, or we may be reloading.
      continue;
    }
    if (!snapshot) continue;
    store.setUploadStates(
      queueId,
      snapshot.upload_states,
      snapshot.published_urls,
    );
    const allTerminal = Object.values(snapshot.upload_states).every(
      (s) => s.state === "published" || s.state === "failed",
    );
    if (allTerminal) return;
  }
}

/** Poll the timeline-render job. */
async function pollVideoJob(
  jobId: string,
  queueId: string,
  outputPath: string,
): Promise<void> {
  const store = useRenderQueueStore.getState();
  while (true) {
    await sleep(POLL_INTERVAL_MS);
    let status: JobStatus;
    try {
      status = await invoke<JobStatus>("poll_timeline_render", { jobId });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      store.markFailed(queueId, message);
      return;
    }
    if (status.progress_pct !== null) {
      store.markProgress(queueId, status.progress_pct);
    }
    if (status.state === "done") {
      store.markDone(queueId, outputPath);
      return;
    }
    if (status.state === "failed") {
      store.markFailed(queueId, status.log_excerpt ?? "render failed");
      return;
    }
    if (status.state === "cancelled") {
      store.markCancelled(queueId);
      return;
    }
  }
}

/** Poll the reframe job via the agent store's Job events. The reframe
 *  command emits `Item::Job` lifecycle events; the agent store records
 *  them and updates each tick. We watch the matching id and mark our
 *  queue entry accordingly. */
async function pollReframeJobViaAgentStore(
  jobId: string,
  queueId: string,
  outputPath: string,
): Promise<void> {
  const { useAgentStore } = await import("../agent/store");
  const store = useRenderQueueStore.getState();
  while (true) {
    await sleep(POLL_INTERVAL_MS);
    const items = useAgentStore.getState().items;
    const match = items.find(
      (it) => it.kind === "job" && it.id.toString() === jobId,
    );
    if (!match || match.kind !== "job") continue;
    if (typeof match.percent === "number") {
      store.markProgress(queueId, match.percent);
    }
    if (match.phase === "completed") {
      const result = match.result;
      if (result === "cancelled") {
        store.markCancelled(queueId);
      } else if (result && typeof result === "object" && "err" in result) {
        store.markFailed(queueId, result.err.message);
      } else if (result && typeof result === "object" && "ok" in result) {
        store.markDone(queueId, match.output_path ?? outputPath);
      } else {
        store.markDone(queueId, match.output_path ?? outputPath);
      }
      return;
    }
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Retry one failed upload for a render. Caller passes the queue
 * entry's `jobId`, the provider key, and the file/title metadata
 * (we re-stub it the same way `maybeChainUploads` does — when W5.A3
 * lands the per-target form, it'll override here too).
 *
 * Kicks the backend and starts a fresh poll loop. Errors land in the
 * target's `failed` state via the standard polling path.
 */
export async function retryUploadForTarget(
  entry: RenderQueueEntry,
  jobId: string,
  provider: string,
): Promise<void> {
  const filePath = entry.outputPath;
  if (!filePath) {
    // Render hasn't completed — nothing to upload. Defensive: the UI
    // should hide the Retry button until outputPath is set.
    return;
  }
  try {
    await invoke<void>("retry_upload", {
      jobId,
      provider,
      filePath,
      title: entry.label,
    });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    const store = useRenderQueueStore.getState();
    const states: Record<string, RenderUploadState> = {
      ...(entry.uploadStates ?? {}),
      [provider]: { state: "failed", reason: message },
    };
    store.setUploadStates(entry.id, states, entry.publishedUrls ?? {});
    return;
  }
  await pollUploadStates(entry.id, jobId);
}
