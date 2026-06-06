// Publish-jobs surface: list jobs with live status + cancel/retry. Firing is
// the server's job now (pg_cron, Phase 4), so there is no client "advance"
// worker — the UI passively polls while any job is non-terminal and shows the
// server-driven status. Talks to the `social_*` Tauri commands; derivation
// lives in `socialModel.ts`.

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";

import {
  jobStatusLabel,
  canCancel,
  canRetry,
  canReschedule,
  isTerminal,
  type PublishJob,
} from "./socialModel";

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

function toDateTimeLocal(secs: number): string {
  const date = new Date(secs * 1000);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function fromDateTimeLocal(value: string): number | null {
  const millis = new Date(value).getTime();
  if (!Number.isFinite(millis)) return null;
  return Math.floor(millis / 1000);
}

export function SocialJobs({ jobIds }: { jobIds: string[] }) {
  const [jobs, setJobs] = useState<Record<string, PublishJob>>({});
  const [rescheduleAt, setRescheduleAt] = useState<Record<string, string>>({});
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

  const reschedule = useCallback(
    async (jobId: string) => {
      const scheduledFor = fromDateTimeLocal(rescheduleAt[jobId] ?? "");
      if (!scheduledFor) {
        setError("Choose a valid reschedule time");
        return;
      }
      try {
        const job = await invoke<PublishJob>("social_reschedule_job", {
          jobId,
          args: { scheduledFor },
        });
        setJobs((prev) => ({ ...prev, [job.id]: job }));
        setRescheduleAt((prev) => ({
          ...prev,
          [job.id]: toDateTimeLocal(job.scheduledFor),
        }));
        setError(null);
      } catch (e) {
        setError(String(e));
      }
    },
    [rescheduleAt],
  );

  const rows = jobIds.map((id) => jobs[id]).filter((j): j is PublishJob => !!j);

  // Initial load.
  useEffect(() => {
    void refreshAll();
  }, [refreshAll]);

  // Passive polling: while any tracked job is still non-terminal the server is
  // (or will be) advancing it, so re-poll every few seconds. Stops once every
  // job is terminal to avoid needless invokes.
  useEffect(() => {
    const anyInFlight = rows.some((job) => !isTerminal(job.status));
    if (!anyInFlight) return;
    const handle = window.setInterval(() => void refreshAll(), 5000);
    return () => window.clearInterval(handle);
  }, [rows, refreshAll]);

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
            {canReschedule(job.status) && (
              <span className="social-jobs__reschedule">
                <input
                  type="datetime-local"
                  value={rescheduleAt[job.id] ?? toDateTimeLocal(job.scheduledFor)}
                  onChange={(event) =>
                    setRescheduleAt((prev) => ({
                      ...prev,
                      [job.id]: event.currentTarget.value,
                    }))
                  }
                  aria-label={`Reschedule ${job.id}`}
                />
                <button type="button" onClick={() => void reschedule(job.id)}>
                  Reschedule
                </button>
              </span>
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
