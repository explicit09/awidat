// Renders Item::Job entries — yt-dlp downloads, local imports,
// indexing runs, transcodes, and timeline exports. Same card layout
// for all five; the JobKind drives the label + per-kind affordances.
//
// Cancel routing: most kinds use the generic `cancel_job` command
// keyed by the job-item id. Render uses its own
// `cancel_timeline_render` because that JobManager is separate from
// the AwidatState::jobs map.
//
// Output-path artifacts: render and transcode set Item::Job's
// `output_path` field on Completed. Render's Completed-Ok shows a
// "Show in Finder" button. Transcode doesn't need one — proxies
// are an internal artifact users don't navigate to manually.

import { invoke } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { Item, JobKind, JobResult } from "../protocol";
import { Tooltip } from "../components/ui/Tooltip";

type Props = {
  /** The Job item this card represents. */
  item: Extract<Item, { kind: "job" }>;
};

export function JobCard({ item }: Props) {
  const isRunning = item.phase !== "completed";
  const labelMap: Record<JobKind, string> = {
    url_import: "url import",
    local_import: "local import",
    transcode: "transcode (proxy)",
    thumbnails: "thumbnails",
    waveform: "waveform",
    indexing: "indexing",
    render: "export",
  };
  const cls = resultClass(item.result);

  function cancel() {
    if (item.job_kind === "render") {
      // Render lives in AwidatState::render_jobs, not the generic
      // jobs map; needs its own cancel command.
      invoke("cancel_timeline_render", { jobId: item.id }).catch(() => {});
    } else {
      invoke("cancel_job", { jobId: item.id }).catch(() => {});
    }
  }

  function showInFinder() {
    if (item.output_path) {
      revealItemInDir(item.output_path).catch((e) => {
        // eslint-disable-next-line no-console
        console.warn("revealItemInDir failed", e);
      });
    }
  }

  const showFinderButton =
    !isRunning &&
    item.job_kind === "render" &&
    item.result !== null &&
    item.result !== "cancelled" &&
    "ok" in item.result &&
    item.output_path !== null &&
    item.output_path !== undefined;

  return (
    <article className={`item item-job item-job-${cls}`}>
      <header className="job-header">
        <div className="item-meta">
          job · <code>{labelMap[item.job_kind]}</code>
        </div>
        {isRunning && (
          <Tooltip content="Stop this job">
            <button className="job-cancel" onClick={cancel}>
              Cancel
            </button>
          </Tooltip>
        )}
        {showFinderButton && (
          <Tooltip content="Reveal the rendered file in Finder">
            <button className="job-action" onClick={showInFinder}>
              Show in Finder
            </button>
          </Tooltip>
        )}
      </header>
      <div className="job-status">{item.status}</div>
      {item.percent !== null && (
        <div className="job-progress">
          <div
            className="job-progress-fill"
            style={{ width: `${item.percent}%` }}
          />
          <span className="job-progress-label">{item.percent}%</span>
        </div>
      )}
    </article>
  );
}

function resultClass(r: JobResult | null): string {
  if (r === null) return "running";
  if (r === "cancelled") return "cancelled";
  if ("ok" in r) return "ok";
  if ("err" in r) return "err";
  return "running";
}
