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
  hasRunnablePendingWithoutRunning,
  reframeMasterPathForEntry,
  renderQueueSelectors,
  sourceDependencyFailure,
  useRenderQueueStore,
  type RenderQueueEntry,
  type RenderUploadState,
} from "./renderQueue";
import {
  useUploadMetadata,
  type UploadMetadata,
} from "../state/uploadMetadata";
import {
  useAiDisclosure,
  type AiDisclosure,
} from "../state/aiDisclosure";
import { publishRenderTargetsViaServer } from "./serverPublish";
import { reasonCopy } from "./social/socialModel";

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

type SocialPublishJob = {
  id: string;
  status: string;
  scheduledFor?: number | null;
  providerPostId?: string | null;
  providerPostUrl?: string | null;
  normalizedError?: string | null;
  requiresActionReason?: string | null;
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
const WORKER_WATCHDOG_INTERVAL_MS = 1_000;

function publishedUrlsFromStates(
  states: Record<string, RenderUploadState>,
): Record<string, string> {
  const urls: Record<string, string> = {};
  for (const [provider, state] of Object.entries(states)) {
    if (state.state === "published" && state.remote_url) {
      urls[provider] = state.remote_url;
    }
  }
  return urls;
}

function uploadStateFromSocialJob(job: SocialPublishJob): RenderUploadState {
  if (job.status === "published") {
    return {
      state: "published",
      remote_id: job.providerPostId ?? job.id,
      ...(job.providerPostUrl ? { remote_url: job.providerPostUrl } : {}),
    };
  }
  if (
    job.status === "failed" ||
    job.status === "requires_action" ||
    job.status === "cancelled"
  ) {
    const reason =
      job.normalizedError ??
      job.requiresActionReason ??
      `server publish ${job.status}`;
    return {
      state: "failed",
      reason: reasonCopy(reason),
      job_id: job.id,
    };
  }
  if (job.status === "processing" || job.status === "uploading") {
    return { state: "processing", job_id: job.id };
  }
  return {
    state: "scheduled",
    job_id: job.id,
    scheduled_for: job.scheduledFor ?? undefined,
  };
}

export async function refreshServerUploadState(
  entry: RenderQueueEntry,
  provider: string,
): Promise<void> {
  const current = entry.uploadStates?.[provider];
  if (
    current?.state !== "scheduled" &&
    current?.state !== "processing" &&
    !(current?.state === "failed" && current.job_id)
  ) {
    return;
  }
  const latest =
    useRenderQueueStore.getState().entries.find((e) => e.id === entry.id)
      ?.uploadStates ?? {};
  let nextState: RenderUploadState;
  try {
    const job = await invoke<SocialPublishJob>("social_publish_job", {
      jobId: current.job_id,
    });
    nextState = uploadStateFromSocialJob(job);
  } catch (error) {
    nextState = {
      state: "failed",
      reason: error instanceof Error ? error.message : String(error),
      job_id: current.job_id,
    };
  }
  const next = { ...latest, [provider]: nextState };
  useRenderQueueStore
    .getState()
    .setUploadStates(entry.id, next, publishedUrlsFromStates(next));
}

/** Hook that runs the queue. Mount it once at the app root (App.tsx);
 *  it idles when there's nothing pending. */
export function useRenderQueueWorker(): void {
  const busyRef = useRef(false);
  const lastMasterPathRef = useRef<string | null>(null);

  useEffect(() => {
    const drainQueue = () => {
      if (busyRef.current) return;
      const pending = renderQueueSelectors.pending(
        useRenderQueueStore.getState(),
      );
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
          queueMicrotask(drainQueue);
        });
    };
    const unsub = useRenderQueueStore.subscribe(drainQueue);
    // Kick the worker once on mount in case there are pending entries
    // already (e.g. after page reload restoring localStorage).
    drainQueue();
    const watchdog = window.setInterval(() => {
      if (!hasRunnablePendingWithoutRunning(useRenderQueueStore.getState().entries)) {
        return;
      }
      busyRef.current = false;
      drainQueue();
    }, WORKER_WATCHDOG_INTERVAL_MS);
    return () => {
      unsub();
      window.clearInterval(watchdog);
    };
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
    const entries = useRenderQueueStore.getState().entries;
    const sourceFailure = sourceDependencyFailure(entry, entries);
    if (sourceFailure) {
      store.markFailed(entry.id, sourceFailure);
      return;
    }
    const master = reframeMasterPathForEntry(
      entry,
      entries,
      lastMasterPathRef.current,
    );
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

/**
 * After a render lands at `done`, fan out uploads to the user's
 * selected server-backed social targets.
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
    store.setUploadStates(
      entry.id,
      Object.fromEntries(targets.map((p) => [p, { state: "pending" }])),
      {},
    );
    store.setUploadMetadata(entry.id, metadataByProvider);
    // Compute the AI disclosure (W5.A4) — backend walks the project
    // timeline against the generated-media registry. The result is
    // parked on the upload-queue entry so every target's
    // `UploadParams` carries the same disclosure when the dispatcher
    // runs. Mirror onto the local entry + the disclosure store so
    // the RenderQueue chip + UploadMetadataForm banner render.
    //
    // When the auto-disclose toggle is OFF (power-user opt-out) we
    // still compute + show the disclosure locally — the user needs
    // to *see* what they're not flagging — but explicitly skip the
    // backend stamp so the upload `UploadParams.ai_disclosure` stays
    // None and providers don't set the platform flag.
    const autoDiscloseEnabled =
      useAiDisclosure.getState().autoDiscloseEnabled;
    const disclosure = await computeAiDisclosure(jobId, autoDiscloseEnabled);
    if (disclosure) {
      store.setAiDisclosure(entry.id, disclosure);
      useAiDisclosure.getState().set(jobId, disclosure);
    }

    await publishRenderTargetsViaServer({
      renderQueueId: entry.id,
      renderJobId: jobId,
      outputPath,
      title: entry.label,
      targets,
      accountIdsByProvider: entry.uploadAccountIds,
      metadataByProvider,
      invoke,
      onState: (provider, state) => {
        const current =
          useRenderQueueStore.getState().entries.find((e) => e.id === entry.id)
            ?.uploadStates ?? {};
        const next = { ...current, [provider]: state };
        const urls = publishedUrlsFromStates(next);
        store.setUploadStates(entry.id, next, urls);
      },
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
      store.markProgress(queueId, status.progress_pct, {
        phase: "rendering_source",
        etaS: status.eta_s,
        timeDoneS: status.time_done_s,
        logExcerpt: status.log_excerpt,
      });
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
      store.markProgress(queueId, match.percent, {
        phase: "rendering_target",
      });
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
 * Ask the backend for the AI disclosure for this render. The backend
 * walks the project's timeline against the generated-media registry
 * + parks the result on the upload-queue entry for the dispatcher.
 *
 * `autoDisclose` mirrors the user's "Auto-disclose AI content" toggle:
 * when true (default) the backend parks the computed disclosure on
 * the queue so the dispatcher stamps it onto every target's
 * `UploadParams`; when false the backend parks an empty disclosure
 * (no auto-flag) but still returns the real computed one for the UI
 * to display — the banner must warn the user about what they're not
 * flagging automatically.
 *
 * Failures (no project, IPC error) collapse to `undefined` — the
 * dispatcher proceeds without a banner; the upload chain stays alive.
 */
async function computeAiDisclosure(
  jobId: string,
  autoDisclose: boolean,
): Promise<AiDisclosure | undefined> {
  try {
    return await invoke<AiDisclosure>("compute_ai_disclosure", {
      jobId,
      autoDisclose,
    });
  } catch {
    return undefined;
  }
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
/**
 * Cancel a running render. Kills the backend ffmpeg child via the matching
 * cancel command (timeline vs reframe), then marks the entry cancelled so the
 * UI reflects it immediately. No-op if the entry has no backend job id yet
 * (still spinning up) — in that case we still mark it cancelled so the worker's
 * poll loop sees a terminal state and stops.
 */
export async function cancelRender(entry: RenderQueueEntry): Promise<void> {
  const store = useRenderQueueStore.getState();
  const jobId = entry.jobId;
  if (jobId) {
    const command =
      entry.kind === "video_reframe"
        ? "cancel_reframe_render"
        : "cancel_timeline_render";
    try {
      await invoke<void>(command, { jobId });
    } catch (err) {
      // Best-effort: even if the kill races (job already finishing), still
      // mark cancelled so the UI isn't stuck "running".
      // eslint-disable-next-line no-console
      console.warn(`${command} failed`, err);
    }
  }
  store.markCancelled(entry.id);
}

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
  const metadata =
    entry.uploadMetadata?.[provider] ??
    useUploadMetadata.getState().get(entry.id, provider, entry.label);
  try {
    await publishRenderTargetsViaServer({
      renderQueueId: entry.id,
      renderJobId: jobId,
      outputPath: filePath,
      title: entry.label,
      targets: [provider],
      accountIdsByProvider: entry.uploadAccountIds,
      metadataByProvider: { [provider]: metadata },
      invoke,
      onState: (_provider, state) => {
        const current =
          useRenderQueueStore.getState().entries.find((e) => e.id === entry.id)
            ?.uploadStates ?? {};
        const next = { ...current, [provider]: state };
        const urls = publishedUrlsFromStates(next);
        useRenderQueueStore
          .getState()
          .setUploadStates(entry.id, next, urls);
      },
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
}
