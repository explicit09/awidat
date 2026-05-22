import { useState } from "react";
import type { ActiveJobLike } from "./activeJobs";
import { JobsPanel } from "./JobsPanel";
import { summarizeJobs } from "./jobsSummary";

type Props = {
  jobs: ActiveJobLike[];
  /** Pre-computed aggregate percent (use `aggregatePercent(jobs)` at
   *  the mount site). `null` hides the percent suffix. */
  totalPercent: number | null;
};

/**
 * Persistent footer pill — summarizes active background work and
 * exposes a click-to-expand panel listing every running job. Idle
 * state renders a muted "Idle" label so the layout never collapses.
 */
export function JobsStatusBar({ jobs, totalPercent }: Props) {
  const [open, setOpen] = useState(false);
  if (jobs.length === 0) {
    return <span className="footer-jobs footer-jobs-idle">Idle</span>;
  }
  return (
    <div className="footer-jobs">
      <button
        type="button"
        className="footer-jobs-pill"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        <span className="footer-jobs-spinner" aria-hidden />
        <span>{summarizeJobs(jobs)}</span>
        {totalPercent !== null && (
          <span className="footer-jobs-percent">{Math.round(totalPercent)}%</span>
        )}
      </button>
      {open && <JobsPanel jobs={jobs} onClose={() => setOpen(false)} />}
    </div>
  );
}
