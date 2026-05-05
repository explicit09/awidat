// Renders Item::Job entries — yt-dlp downloads, local imports, and
// indexing runs. Same card layout for all three; the JobKind drives
// the icon + label.

import { invoke } from "@tauri-apps/api/core";
import type { Item, JobKind, JobResult } from "../protocol";

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
    indexing: "indexing",
  };
  const cls = resultClass(item.result);

  function cancel() {
    invoke("cancel_job", { jobId: item.id }).catch(() => {});
  }

  return (
    <article className={`item item-job item-job-${cls}`}>
      <header className="job-header">
        <div className="item-meta">
          job · <code>{labelMap[item.job_kind]}</code>
        </div>
        {isRunning && (
          <button className="job-cancel" onClick={cancel}>
            Cancel
          </button>
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
