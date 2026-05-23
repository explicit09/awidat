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
} from "./renderQueue";

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
