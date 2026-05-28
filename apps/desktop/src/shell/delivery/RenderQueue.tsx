import {
  Card,
  Inline,
  Stack,
  StatusPill,
} from "../../ui";
import {
  useRenderQueueStore,
  type RenderQueueEntry,
} from "../../app/renderQueue";

/**
 * Right column — live render queue.
 *
 * Empty state: a calm "No renders queued. Pick targets and hit
 * Export." card with the same header chrome so the section still
 * holds visual space without screaming "nothing here".
 *
 * Non-empty: each entry is a row with the label on the left and a
 * `<StatusPill>` on the right that surfaces (running %, ready,
 * failed, idle). Running rows also draw a thin brand-colored bar
 * underneath for at-a-glance progress, and terminal rows expose
 * "Open in Finder" + review actions.
 */
export function RenderQueuePanel() {
  const entries = useRenderQueueStore((s) => s.entries);
  const dismiss = useRenderQueueStore((s) => s.dismiss);
  const clearTerminal = useRenderQueueStore((s) => s.clearTerminal);
  const markReviewed = useRenderQueueStore((s) => s.markReviewed);
  const visible = entries.slice(-12);
  if (visible.length === 0) {
    return (
      <Card padding="md">
        <Stack gap="2">
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
            Render queue
          </span>
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
            No renders queued. Pick targets and hit Export.
          </span>
        </Stack>
      </Card>
    );
  }
  const hasTerminal = visible.some(
    (e) =>
      e.status === "done" || e.status === "failed" || e.status === "cancelled",
  );
  return (
    <Card padding="md">
      <Stack gap="3">
        <Inline justify="between" align="baseline">
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
            Render queue
          </span>
          {hasTerminal ? (
            <button
              type="button"
              onClick={clearTerminal}
              className="text-[var(--text-caption)] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]"
            >
              Clear done
            </button>
          ) : null}
        </Inline>
        <Stack gap="2">
          {visible.map((entry) => (
            <RenderQueueRow
              key={entry.id}
              entry={entry}
              onDismiss={() => dismiss(entry.id)}
              onReview={(reviewStatus) => markReviewed(entry.id, reviewStatus)}
            />
          ))}
        </Stack>
      </Stack>
    </Card>
  );
}

/** Map a queue entry's status to a `<StatusPill>` family+state pair.
 *  `pending` reads as idle; `done` as ready; `failed`/`cancelled` as
 *  failed; `running` keeps its percent so the pill shows progress. */
function queueStatusPill(entry: RenderQueueEntry) {
  if (entry.status === "running") {
    return (
      <StatusPill
        family="job"
        state="running"
        size="sm"
        percent={typeof entry.progress === "number" ? entry.progress : 0}
      />
    );
  }
  if (entry.status === "done") {
    return (
      <StatusPill
        family="job"
        state="ready"
        size="sm"
        label={entry.reviewStatus === "pending" ? "Review" : "Done"}
      />
    );
  }
  if (entry.status === "failed" || entry.status === "cancelled") {
    return (
      <StatusPill
        family="job"
        state="failed"
        size="sm"
        label={entry.status === "cancelled" ? "Cancelled" : "Failed"}
      />
    );
  }
  return <StatusPill family="job" state="idle" size="sm" label="Queued" />;
}

function RenderQueueRow({
  entry,
  onDismiss,
  onReview,
}: {
  entry: RenderQueueEntry;
  onDismiss: () => void;
  onReview: (
    reviewStatus: NonNullable<RenderQueueEntry["reviewStatus"]>,
  ) => void;
}) {
  return (
    <div className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2 text-[var(--text-caption)]">
      <Inline justify="between" align="center" className="gap-2">
        <span className="min-w-0 truncate font-medium text-[var(--color-text-primary)]">
          {entry.label}
        </span>
        <Inline gap="2" align="center" className="shrink-0">
          {queueStatusPill(entry)}
          {entry.status === "done" || entry.status === "failed" || entry.status === "cancelled" ? (
            <button
              type="button"
              onClick={onDismiss}
              className="text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]"
              title="Dismiss"
            >
              ×
            </button>
          ) : null}
        </Inline>
      </Inline>
      {entry.status === "running" && typeof entry.progress === "number" ? (
        <div className="mt-2 h-1 overflow-hidden rounded-full bg-[var(--color-surface-input)]">
          <div
            className="h-full rounded-full bg-[var(--color-brand)]"
            style={{ width: `${entry.progress}%` }}
          />
        </div>
      ) : null}
      {entry.status === "done" && entry.outputPath ? (
        <div className="mt-2 space-y-2">
          <button
            type="button"
            onClick={() => void invokeRevealInFinder(entry.outputPath!)}
            className="truncate text-[var(--color-brand-secondary)] hover:underline"
            title={entry.outputPath}
          >
            Open in Finder
          </button>
          {entry.reviewStatus === "pending" ? (
            <div className="grid grid-cols-2 gap-1.5">
              <button
                type="button"
                onClick={() => onReview("approved")}
                className="rounded-[var(--radius-sm)] border border-[rgba(32,201,151,0.45)] bg-[rgba(32,201,151,0.12)] px-2 py-1 text-[var(--color-success)] hover:bg-[rgba(32,201,151,0.18)]"
              >
                Looks good
              </button>
              <button
                type="button"
                onClick={() => onReview("changes_requested")}
                className="rounded-[var(--radius-sm)] border border-[rgba(245,158,11,0.45)] bg-[rgba(245,158,11,0.1)] px-2 py-1 text-[var(--color-warning)] hover:bg-[rgba(245,158,11,0.16)]"
              >
                Needs changes
              </button>
            </div>
          ) : entry.reviewStatus === "approved" ? (
            <p className="text-[var(--color-success)]">Approved for delivery.</p>
          ) : entry.reviewStatus === "changes_requested" ? (
            <p className="text-[var(--color-warning)]">Changes requested. Re-edit before delivery.</p>
          ) : null}
        </div>
      ) : null}
      {entry.status === "failed" && entry.error ? (
        <p className="mt-1 truncate text-[var(--color-job-failed-text)]" title={entry.error}>
          {entry.error}
        </p>
      ) : null}
    </div>
  );
}

async function invokeRevealInFinder(path: string): Promise<void> {
  try {
    const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
    await revealItemInDir(path);
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn("revealItemInDir failed", err);
  }
}
