// Per-account audit surface: status counts, job list with final URLs, and the
// event trail. Talks to `social_account_audit`; derivation in `socialModel.ts`.

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";

import {
  jobStatusLabel,
  type AccountUsageAudit,
} from "./socialModel";

export function SocialAudit({ accountId }: { accountId: string }) {
  const [audit, setAudit] = useState<AccountUsageAudit | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setAudit(
        await invoke<AccountUsageAudit>("social_account_audit", { accountId }),
      );
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [accountId]);

  useEffect(() => {
    void load();
  }, [load]);

  if (error) {
    return (
      <section className="social-audit">
        <p role="alert">{error}</p>
      </section>
    );
  }

  if (!audit) {
    return (
      <section className="social-audit">
        <p>Loading audit…</p>
      </section>
    );
  }

  const counts = audit.statusCounts;

  return (
    <section className="social-audit">
      <header className="social-audit__header">Account usage</header>

      <dl className="social-audit__counts">
        <div>
          <dt>Scheduled</dt>
          <dd>{counts.scheduled}</dd>
        </div>
        <div>
          <dt>Processing</dt>
          <dd>{counts.processing}</dd>
        </div>
        <div>
          <dt>Published</dt>
          <dd>{counts.published}</dd>
        </div>
        <div>
          <dt>Failed</dt>
          <dd>{counts.failed}</dd>
        </div>
        <div>
          <dt>Action needed</dt>
          <dd>{counts.requiresAction}</dd>
        </div>
      </dl>

      <ul className="social-audit__jobs">
        {audit.jobs.map((job) => (
          <li key={job.id} className="social-audit__job">
            <span
              className="status-dot"
              data-status={job.status}
              aria-hidden="true"
            />
            <span>{job.id}</span>
            <span>{jobStatusLabel(job.status)}</span>
            {job.providerPostUrl && (
              <button
                type="button"
                onClick={() => void openUrl(job.providerPostUrl as string)}
              >
                View post
              </button>
            )}
          </li>
        ))}
      </ul>

      <ol className="social-audit__events">
        {audit.events.map((ev) => (
          <li key={ev.id}>
            <span className="social-audit__event-type">{ev.eventType}</span>
            <span>{ev.message}</span>
          </li>
        ))}
      </ol>
    </section>
  );
}
