import type { ActiveJobLike } from "./activeJobs";
import { jobKindLabel } from "./jobsSummary";

type Props = {
  jobs: ActiveJobLike[];
  onClose: () => void;
};

/**
 * Expanded panel showing each active background job. Rendered as a
 * popover above the footer pill when the user clicks the pill.
 */
export function JobsPanel({ jobs, onClose }: Props) {
  return (
    <div className="footer-jobs-panel" role="dialog">
      <header>
        <strong>Background work</strong>
        <button type="button" onClick={onClose} aria-label="Close">
          ×
        </button>
      </header>
      {jobs.length === 0 ? (
        <p className="footer-jobs-panel-empty">No active jobs.</p>
      ) : (
        <ul>
          {jobs.map((job) => (
            <li key={job.id}>
              <span className="footer-jobs-row-kind">{jobKindLabel(job.kind)}</span>
              <span className="footer-jobs-row-status" title={job.status}>
                {job.status}
              </span>
              <span className="footer-jobs-row-pct">{Math.round(job.percent)}%</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
