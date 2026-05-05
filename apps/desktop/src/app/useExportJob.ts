// Polling-driven Export job. Manages the lifecycle of one timeline
// render: start_timeline_render → 500ms-poll until terminal state →
// synthesize Item::Job events into the agent store along the way.
//
// We synthesize on the frontend rather than push from the backend
// because the backend's render path doesn't go through Session,
// has no broadcast subscriber, and the polling cadence is fine for
// a progress bar — pushing every ffmpeg progress line would be
// noisier than useful.

import { useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useAgentStore } from "../agent/store";
import type { Item } from "../protocol";

/** Subset of `awidat_render::JobStatus` we actually consume. */
type JobStatus = {
  id: string;
  state: "queued" | "running" | "done" | "failed" | "cancelled";
  progress_pct: number | null;
  total_duration_s: number | null;
  time_done_s: number | null;
  eta_s: number | null;
  log_excerpt: string;
  output_path: string;
  exit_code: number | null;
};

type StartResult = {
  job_id: string;
  output_path: string;
  total_duration_s: number | null;
};

type Job = Extract<Item, { kind: "job" }>;

const POLL_INTERVAL_MS = 500;

export function useExportJob() {
  const upsert = useAgentStore((s) => s.upsert);
  // Track the active interval so we can clear it on unmount or on
  // a second start. One job at a time per hook instance.
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Stop polling on unmount so we don't keep firing IPC calls into
  // a torn-down view.
  useEffect(() => {
    return () => {
      if (intervalRef.current !== null) {
        clearInterval(intervalRef.current);
        intervalRef.current = null;
      }
    };
  }, []);

  const start = useCallback(async (): Promise<void> => {
    if (intervalRef.current !== null) {
      // Already polling a render. Caller should have disabled the
      // button via runningKinds; defensive bail.
      return;
    }

    let info: StartResult;
    try {
      info = await invoke<StartResult>("start_timeline_render");
    } catch (e) {
      // Show a one-shot Completed-Err item so the failure is visible
      // even though no Started ever fired.
      upsert(
        makeJob({
          id: `render-${Date.now()}-err`,
          phase: "completed",
          status: `failed to start render: ${stringifyErr(e)}`,
          result: { err: { message: String(e) } },
          output_path: null,
        }),
      );
      return;
    }

    const { job_id, output_path } = info;

    upsert(
      makeJob({
        id: job_id,
        phase: "started",
        status: "starting render…",
        result: null,
        output_path: null,
        // total_duration_s unused on the Started phase.
      }),
    );

    intervalRef.current = setInterval(async () => {
      let st: JobStatus;
      try {
        st = await invoke<JobStatus>("poll_timeline_render", { jobId: job_id });
      } catch (e) {
        // Polling itself failed — surface as terminal err and stop.
        clearIntervalSafe(intervalRef);
        upsert(
          makeJob({
            id: job_id,
            phase: "completed",
            status: `poll failed: ${stringifyErr(e)}`,
            result: { err: { message: String(e) } },
            output_path: null,
          }),
        );
        return;
      }

      switch (st.state) {
        case "queued":
        case "running": {
          const pct =
            st.progress_pct === null
              ? null
              : Math.max(0, Math.min(100, Math.round(st.progress_pct)));
          upsert(
            makeJob({
              id: job_id,
              phase: "delta",
              status: formatRunningStatus(st),
              result: null,
              output_path: null,
              percent: pct,
            }),
          );
          break;
        }
        case "done": {
          clearIntervalSafe(intervalRef);
          upsert(
            makeJob({
              id: job_id,
              phase: "completed",
              status: `wrote ${output_path}`,
              result: { ok: { summary: `wrote ${output_path}` } },
              output_path,
              percent: 100,
            }),
          );
          break;
        }
        case "failed": {
          clearIntervalSafe(intervalRef);
          const tail = lastNonEmptyLines(st.log_excerpt, 200);
          upsert(
            makeJob({
              id: job_id,
              phase: "completed",
              status: tail || "render failed",
              result: { err: { message: tail || "render failed" } },
              output_path: null,
            }),
          );
          break;
        }
        case "cancelled": {
          clearIntervalSafe(intervalRef);
          upsert(
            makeJob({
              id: job_id,
              phase: "completed",
              status: "cancelled",
              result: "cancelled",
              output_path: null,
            }),
          );
          break;
        }
      }
    }, POLL_INTERVAL_MS);
  }, [upsert]);

  return { start };
}

function makeJob(args: {
  id: string;
  phase: Job["phase"];
  status: string;
  result: Job["result"];
  output_path: string | null;
  percent?: number | null;
}): Job {
  return {
    kind: "job",
    id: args.id,
    phase: args.phase,
    job_kind: "render",
    percent: args.percent ?? null,
    status: args.status,
    result: args.result,
    output_path: args.output_path,
  };
}

function formatRunningStatus(st: JobStatus): string {
  // "rendered 0:23 of 0:56 · ~12s remaining"
  const cur = st.time_done_s !== null ? formatTime(st.time_done_s) : "—";
  const total =
    st.total_duration_s !== null ? formatTime(st.total_duration_s) : "—";
  const eta =
    st.eta_s !== null && Number.isFinite(st.eta_s)
      ? ` · ~${Math.round(st.eta_s)}s remaining`
      : "";
  return `rendered ${cur} of ${total}${eta}`;
}

function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

/** Return the last `maxChars` characters from the most recent
 *  non-empty line of `text`. ffmpeg's stderr last line is usually
 *  the actual error reason. */
function lastNonEmptyLines(text: string, maxChars: number): string {
  const lines = text
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l.length > 0);
  const last = lines[lines.length - 1] ?? "";
  return last.length > maxChars ? `…${last.slice(-maxChars)}` : last;
}

function stringifyErr(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

function clearIntervalSafe(
  ref: React.MutableRefObject<ReturnType<typeof setInterval> | null>,
) {
  if (ref.current !== null) {
    clearInterval(ref.current);
    ref.current = null;
  }
}
