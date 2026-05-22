// Pure summarization helpers for the footer JobsStatusBar pill.
//
// Split out so the visible text can be unit-tested without needing
// React DOM Server in the existing Node test runner.

import type { ActiveJobLike } from "./activeJobs";

/**
 * Short label for a job-kind value as it should appear in the footer
 * pill. Unknown kinds pass through unchanged so we never silently
 * drop a job from the count.
 */
export function jobKindLabel(kind: string): string {
  switch (kind) {
    case "transcode":   return "transcoding";
    case "indexing":    return "indexing";
    case "thumbnails":  return "thumbnailing";
    case "waveform":    return "waveform";
    case "silences":    return "silence detection";
    case "motion":      return "motion analysis";
    case "render":      return "rendering";
    case "local_import":
    case "url_import":  return "importing";
    default:            return kind;
  }
}

/**
 * One-line pill text. Groups jobs by kind, counts them, joins with
 * commas. "Idle" when there are no active jobs.
 */
export function summarizeJobs(jobs: ActiveJobLike[]): string {
  if (jobs.length === 0) return "Idle";
  const counts = new Map<string, number>();
  for (const job of jobs) counts.set(job.kind, (counts.get(job.kind) ?? 0) + 1);
  const parts: string[] = [];
  for (const [kind, count] of counts) {
    parts.push(`${count} ${jobKindLabel(kind)}`);
  }
  return parts.join(", ");
}
