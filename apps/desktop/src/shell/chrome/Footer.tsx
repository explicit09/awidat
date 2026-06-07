import { useAgentStore } from "../../agent/store";
import { useRenderQueueStore } from "../../app/renderQueue";
import { StatusPill } from "../../ui/primitives/StatusPill";
import { toActiveJobLike, aggregatePercent } from "../activeJobs";

/**
 * Footer — redesigned status bar (Task 10 of the v2 redesign).
 *
 * Reads the SAME data that the old <JobsStatusBar /> read — `useAgentStore`
 * for the active-job list — but lays it out as two semantic mono groups:
 *
 *   left   : job state (running pill w/ aggregate %, OR ready pill) + signals
 *   right  : autosaved time · render queue depth · throughput bars · disk free
 *
 * Fields the old footer rendered that have intentionally been dropped here
 * (per Task 10 spec — they move to Settings):
 *   - "Agent online / Agent working" liveness dot
 *   - "Model: Montage Pro 1.2"
 *   - "Context window: local"
 *
 * The outer flex/border/bg layout is supplied by the AppShell's <footer>
 * wrapper, so this component just returns the two sibling groups.
 */
export function Footer() {
  const items = useAgentStore((s) => s.items);
  const renderEntries = useRenderQueueStore((s) => s.entries);

  // Same derivation the old in-App Footer used to feed JobsStatusBar.
  const activeJobs = items.filter(
    (item) => item.kind === "job" && (item as { phase?: string }).phase !== "completed",
  );
  const jobsForBar = activeJobs.map((it) =>
    toActiveJobLike({
      id: it.id,
      job_kind: (it as { job_kind?: string }).job_kind ?? "",
      status: (it as { status?: string }).status,
      percent: (it as { percent?: number }).percent,
    }),
  );
  const indexing = jobsForBar.length > 0;
  const percent = aggregatePercent(jobsForBar) ?? undefined;

  // Render queue depth = pending + running entries (terminal entries excluded).
  const queueDepth = renderEntries.filter(
    (e) => e.status === "pending" || e.status === "running",
  ).length;

  return (
    <>
      <div className="flex items-center gap-4 min-w-0 text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
        {indexing ? (
          <>
            <StatusPill
              family="job"
              state="running"
              percent={percent}
              label="indexing"
              size="sm"
            />
            {/* TODO(redesign): wire `ready / total signals` from a future
                shared store; index_readiness is currently local to App.tsx. */}
            <span className="truncate">— / — signals</span>
          </>
        ) : (
          <StatusPill family="job" state="ready" label="ready" size="sm" />
        )}
      </div>
      <div className="flex items-center gap-4 min-w-0 justify-end text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
        {/* TODO(redesign): wire `lastSavedAt` from autosave state. */}
        <span className="shrink-0">Autosaved · {formatClock(null)}</span>
        <span className="shrink-0">
          render <b className="text-[var(--color-text-secondary)]">{queueDepth}</b>
        </span>
        <ThroughputBars />
        {/* TODO(redesign): wire `freeBytes` from a future disk-stats store. */}
        <span className="shrink-0">
          disk <b className="text-[var(--color-text-secondary)]">local</b>
        </span>
      </div>
    </>
  );
}

function formatClock(d: Date | null): string {
  if (!d) return "—";
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

/**
 * Throughput bars — static 4-bar visual placeholder. The old footer used a
 * static 5-bar visual too (no hook to wire to); the redesign keeps the
 * decorative cue but trims to 4 bars per spec.
 */
function ThroughputBars() {
  return (
    <span className="inline-flex items-end gap-px h-2 shrink-0" aria-hidden>
      <i className="block w-px h-[30%] bg-[var(--color-text-disabled)]" />
      <i className="block w-px h-[55%] bg-[var(--color-text-disabled)]" />
      <i className="block w-px h-[80%] bg-[var(--color-text-disabled)]" />
      <i className="block w-px h-full bg-[var(--color-text-disabled)]" />
    </span>
  );
}
