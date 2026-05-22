// Pure adapter + aggregator for the JobsStatusBar footer pill.
//
// `App.tsx` already memoizes `activeJobs` (the Item::Job stream filtered
// to non-completed entries). The footer status bar wants a flatter shape
// keyed on `kind` / `percent` / `status`. Rather than lifting state into
// a Zustand store, we expose this adapter so the existing memo can pipe
// through it inline at the mount site.

/**
 * Minimal shape of an Item::Job element relevant to the footer pill.
 * Wider than the full Item::Job variant; we only read the fields the
 * adapter needs.
 */
export type RawJobItem = {
  id: string;
  job_kind: string;
  status: string | null | undefined;
  percent: number | null | undefined;
};

/**
 * What the JobsStatusBar consumes. Flat, never-null fields so the
 * component doesn't have to guard.
 */
export type ActiveJobLike = {
  id: string;
  kind: string;
  percent: number;
  status: string;
};

/** Adapt one raw Item::Job into the status-bar shape. */
export function toActiveJobLike(item: RawJobItem): ActiveJobLike {
  return {
    id: item.id,
    kind: item.job_kind,
    percent: typeof item.percent === "number" && Number.isFinite(item.percent)
      ? item.percent
      : 0,
    status: item.status ?? "",
  };
}

/**
 * Mean of the active jobs' percents, or `null` when the list is empty.
 * The status bar uses this to render a single roll-up percentage; the
 * panel renders per-job percents independently.
 */
export function aggregatePercent(jobs: ActiveJobLike[]): number | null {
  if (jobs.length === 0) return null;
  const sum = jobs.reduce((s, j) => s + (Number.isFinite(j.percent) ? j.percent : 0), 0);
  return sum / jobs.length;
}
