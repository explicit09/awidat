// Publish-jobs surface: list jobs with live status, cancel/retry, and a
// worker "advance" action that drives scheduled -> uploading -> processing ->
// published via the worker commands (mock adapters this pass). Talks to the
// `social_*` Tauri commands; derivation lives in `socialModel.ts`.

import { useCallback, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";

import {
  jobStatusLabel,
  canCancel,
  canRetry,
  nextWorkerAction,
  type PublishJob,
} from "./socialModel";

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

export function SocialJobs({ jobIds }: { jobIds: string[] }) {
  const [jobs, setJobs] = useState<Record<string, PublishJob>>({});
  const [error, setError] = useState<string | null>(null);

  const loadJob = useCallback(async (jobId: string) => {
    try {
      const job = await invoke<PublishJob>("social_publish_job", { jobId });
      setJobs((prev) => ({ ...prev, [job.id]: job }));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const refreshAll = useCallback(async () => {
    await Promise.all(jobIds.map((id) => loadJob(id)));
  }, [jobIds, loadJob]);

  const cancel = useCallback(
    async (jobId: string) => {
      try {
        const job = await invoke<PublishJob>("social_cancel_job", {
          jobId,
          now: nowSeconds(),
        });
        setJobs((prev) => ({ ...prev, [job.id]: job }));
      } catch (e) {
        setError(String(e));
      }
    },
    [],
  );

  const retry = useCallback(
    async (jobId: string) => {
      try {
        const job = await invoke<PublishJob>("social_retry_job", {
          jobId,
          now: nowSeconds(),
        });
        setJobs((prev) => ({ ...prev, [job.id]: job }));
      } catch (e) {
        setError(String(e));
      }
    },
    [],
  );

  const advance = useCallback(
    async (job: PublishJob) => {
      const action = nextWorkerAction(job.status);
      if (!action) return;
      try {
        const next =
          action === "execute"
            ? await invoke<PublishJob>("social_execute_upload", {
                args: {
                  jobId: job.id,
                  title: `Awidat ${job.campaignId}`,
                  description: null,
                  tags: [],
                  thumbnailRef: null,
                  now: nowSeconds(),
                },
              })
            : await invoke<PublishJob>("social_poll_status", {
                jobId: job.id,
                now: nowSeconds(),
              });
        setJobs((prev) => ({ ...prev, [next.id]: next }));
      } catch (e) {
        setError(String(e));
      }
    },
    [],
  );

  const rows = jobIds.map((id) => jobs[id]).filter((j): j is PublishJob => !!j);

  return (
    <section className="social-jobs">
      <header className="social-jobs__header">
        <span>Publish jobs</span>
        <button type="button" onClick={() => void refreshAll()}>
          Refresh
        </button>
      </header>

      {error && (
        <p className="social-jobs__error" role="alert">
          {error}
        </p>
      )}

      <ul className="social-jobs__list">
        {rows.map((job) => (
          <li key={job.id} className="social-jobs__row">
            <span
              className="status-dot"
              data-status={job.status}
              aria-hidden="true"
            />
            <span className="social-jobs__id">{job.id}</span>
            <span className="social-jobs__status">
              {jobStatusLabel(job.status)}
            </span>
            {job.providerPostUrl && (
              <button
                type="button"
                className="social-jobs__link"
                onClick={() => void openUrl(job.providerPostUrl as string)}
              >
                View post
              </button>
            )}
            {nextWorkerAction(job.status) && (
              <button type="button" onClick={() => void advance(job)}>
                Advance
              </button>
            )}
            {canCancel(job.status) && (
              <button type="button" onClick={() => void cancel(job.id)}>
                Cancel
              </button>
            )}
            {canRetry(job.status) && (
              <button type="button" onClick={() => void retry(job.id)}>
                Retry
              </button>
            )}
          </li>
        ))}
        {rows.length === 0 && !error && (
          <li className="social-jobs__empty">No jobs to show.</li>
        )}
      </ul>
    </section>
  );
}
