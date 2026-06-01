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

import { useState } from "react";
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
  const [copied, setCopied] = useState(false);
  const [renderReview, setRenderReview] = useState<
    "pending" | "approved" | "changes_requested"
  >("pending");
  const labelMap: Record<JobKind, string> = {
    url_import: "url import",
    local_import: "local import",
    transcode: "transcode (proxy)",
    thumbnails: "thumbnails",
    waveform: "waveform",
    silences: "silences",
    motion: "motion signal",
    indexing: "indexing",
    generated_media: "generated media",
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

  const errorMessage =
    item.result !== null &&
    item.result !== "cancelled" &&
    typeof item.result === "object" &&
    "err" in item.result
      ? item.result.err.message
      : null;

  function copyLog() {
    if (!errorMessage) return;
    navigator.clipboard
      .writeText(errorMessage)
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1200);
      })
      .catch((e) => {
        // eslint-disable-next-line no-console
        console.warn("clipboard.writeText failed", e);
      });
  }

  return (
    <article className={`item item-job item-job-${cls} glass-content`}>
      <header className="job-header">
        <div className="item-meta">
          job · <code>{labelMap[item.job_kind]}</code>
          <span className={`job-chip job-chip-${cls}`}>{jobChipLabel(cls)}</span>
        </div>
        {isRunning && (
          <Tooltip content="Stop this job">
            <button className="job-cancel glass-ghost" onClick={cancel}>
              Cancel
            </button>
          </Tooltip>
        )}
        {showFinderButton && (
          <Tooltip content="Reveal the rendered file in Finder">
            <button className="job-action glass-ghost" onClick={showInFinder}>
              Show in Finder
            </button>
          </Tooltip>
        )}
        {errorMessage && (
          <Tooltip content="Copy the error message to your clipboard">
            <button className="job-action glass-ghost" onClick={copyLog}>
              {copied ? "Copied" : "Copy Log"}
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
      {showFinderButton && (
        <div className="approval-actions">
          {renderReview === "pending" ? (
            <>
              <button
                className="glass-cta"
                onClick={() => setRenderReview("approved")}
              >
                Looks good
              </button>
              <button
                className="deny glass-ghost"
                onClick={() => setRenderReview("changes_requested")}
              >
                Needs changes
              </button>
            </>
          ) : (
            <span className="approval-summary">
              {renderReview === "approved"
                ? "Render approved for delivery."
                : "Changes requested before delivery."}
            </span>
          )}
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

/** Short uppercase label for the status chip, keyed off resultClass. */
function jobChipLabel(cls: string): string {
  switch (cls) {
    case "ok":
      return "done";
    case "err":
      return "failed";
    case "cancelled":
      return "cancelled";
    default:
      return "running";
  }
}
