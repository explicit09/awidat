import { useEffect } from "react";
import {
  Card,
  Inline,
  Stack,
  StatusPill,
} from "../../ui";
import {
  useRenderQueueStore,
  type RenderQueueEntry,
  type RenderUploadState,
} from "../../app/renderQueue";
import {
  cancelRender,
  refreshServerUploadState,
  retryUploadForTarget,
} from "../../app/useRenderQueueWorker";
import { summarizeCredit } from "../../state/aiDisclosure";
import { TARGET_META } from "./targetMeta";
import type { DeliveryTargetKey } from "./types";

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
  useEffect(() => {
    const liveUploadEntries = visible.flatMap((entry) =>
      Object.entries(entry.uploadStates ?? {})
        .filter(
          ([, state]) =>
            state.state === "scheduled" || state.state === "processing",
        )
        .map(([provider]) => ({ entry, provider })),
    );
    if (liveUploadEntries.length === 0) return;
    const tick = () => {
      for (const { entry, provider } of liveUploadEntries) {
        void refreshServerUploadState(entry, provider);
      }
    };
    tick();
    const timer = window.setInterval(tick, 2_000);
    return () => window.clearInterval(timer);
  }, [visible]);

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
        <Inline gap="2" align="center" className="min-w-0">
          <span className="min-w-0 truncate font-medium text-[var(--color-text-primary)]">
            {entry.label}
          </span>
          <AiDisclosureChip entry={entry} />
        </Inline>
        <Inline gap="2" align="center" className="shrink-0">
          {queueStatusPill(entry)}
          {entry.status === "running" || entry.status === "pending" ? (
            <button
              type="button"
              onClick={() => void cancelRender(entry)}
              className="rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] px-1.5 py-0.5 text-[var(--text-caption)] text-[var(--color-text-muted)] hover:border-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
              title="Cancel render"
            >
              Cancel
            </button>
          ) : null}
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
      <UploadTargetsBlock entry={entry} />
    </div>
  );
}

/**
 * Per-target upload state block. Renders one row per registered
 * upload target with its current state — a small arrow + provider
 * label + status copy.
 *
 *   → Uploading to YouTube · 45%
 *   → Published on YouTube  ↗   (link)
 *   → YouTube upload failed: not_configured    [Retry]
 *
 * Hides entirely when there are no upload targets so existing
 * captions / cover / custom rows aren't visually padded.
 */
function UploadTargetsBlock({
  entry,
}: {
  entry: RenderQueueEntry;
}) {
  const targets = entry.uploadTargets ?? [];
  if (targets.length === 0) return null;
  return (
    <ul className="mt-2 space-y-1 text-[var(--text-caption)]">
      {targets.map((provider) => {
        const state: RenderUploadState =
          entry.uploadStates?.[provider] ?? { state: "pending" };
        return (
          <UploadTargetRow
            key={provider}
            entry={entry}
            provider={provider}
            state={state}
          />
        );
      })}
    </ul>
  );
}

/** Look up a provider key's display name. Falls back to the raw key
 *  for unknown providers (defense — new providers may land before the
 *  meta table is updated). */
function providerLabel(provider: string): string {
  const meta = TARGET_META[provider as DeliveryTargetKey];
  return meta?.label ?? provider;
}

function UploadTargetRow({
  entry,
  provider,
  state,
}: {
  entry: RenderQueueEntry;
  provider: string;
  state: RenderUploadState;
}) {
  const label = providerLabel(provider);
  if (state.state === "pending") {
    return (
      <li className="flex items-center gap-1 text-[var(--color-text-muted)]">
        <span aria-hidden>→</span>
        <span>Queued for {label}</span>
      </li>
    );
  }
  if (state.state === "uploading") {
    const pct = Math.round(
      Math.max(0, Math.min(1, Number.isFinite(state.progress) ? state.progress : 0)) *
        100,
    );
    return (
      <li className="flex items-center gap-1 text-[var(--color-text-secondary)]">
        <span aria-hidden>→</span>
        <span>
          Uploading to {label} · {pct}%
        </span>
      </li>
    );
  }
  if (state.state === "scheduled") {
    return (
      <li className="flex flex-wrap items-center gap-1 text-[var(--color-text-secondary)]">
        <span aria-hidden>→</span>
        <span className="min-w-0">Staged for {label} · waiting for server</span>
        <button
          type="button"
          onClick={() => void refreshServerUploadState(entry, provider)}
          className="ml-1 inline-flex items-center rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] px-1.5 py-0.5 text-[var(--text-caption)] text-[var(--color-text-secondary)] hover:border-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
        >
          Refresh
        </button>
      </li>
    );
  }
  if (state.state === "processing") {
    return (
      <li className="flex flex-wrap items-center gap-1 text-[var(--color-text-secondary)]">
        <span aria-hidden>→</span>
        <span className="min-w-0">Processing on {label}</span>
        <button
          type="button"
          onClick={() => void refreshServerUploadState(entry, provider)}
          className="ml-1 inline-flex items-center rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] px-1.5 py-0.5 text-[var(--text-caption)] text-[var(--color-text-secondary)] hover:border-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
        >
          Refresh
        </button>
      </li>
    );
  }
  if (state.state === "published") {
    return (
      <li className="flex items-center gap-1 text-[var(--color-success)]">
        <span aria-hidden>→</span>
        <a
          href={state.remote_url}
          target="_blank"
          rel="noopener noreferrer"
          className="hover:underline"
          title={state.remote_url}
        >
          Published on {label} ↗
        </a>
      </li>
    );
  }
  // Failed — show the reason + recovery actions. A server-backed
  // failure can still refresh if the backend job later recovers.
  const canRetry = Boolean(entry.jobId && entry.outputPath);
  const canRefresh = Boolean(state.job_id);
  return (
    <li className="flex items-start gap-1 text-[var(--color-job-failed-text)]">
      <span aria-hidden>→</span>
      <div className="min-w-0 flex-1">
        <span className="truncate" title={state.reason}>
          {label} upload failed: {state.reason}
        </span>
        {canRefresh ? (
          <button
            type="button"
            onClick={() => void refreshServerUploadState(entry, provider)}
            className="ml-1 inline-flex items-center rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] px-1.5 py-0.5 text-[var(--text-caption)] text-[var(--color-text-secondary)] hover:border-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
          >
            Refresh
          </button>
        ) : null}
        {canRetry ? (
          <button
            type="button"
            onClick={() => {
              if (entry.jobId) {
                void retryUploadForTarget(entry, entry.jobId, provider);
              }
            }}
            className="ml-1 inline-flex items-center rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] px-1.5 py-0.5 text-[var(--text-caption)] text-[var(--color-text-secondary)] hover:border-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
          >
            Retry
          </button>
        ) : null}
      </div>
    </li>
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

/**
 * Small `AI ⚠` chip surfaced next to the render label when the entry's
 * disclosure flags synthetic content (W5.A4). The native `title`
 * attribute carries the full credits list so hover surfaces what's
 * being claimed without a custom tooltip component.
 *
 * Renders nothing when the render is clean (no disclosure, or
 * disclosure has zero credits) — empty cuts shouldn't get a chip.
 */
export function AiDisclosureChip({
  entry,
}: {
  entry: RenderQueueEntry;
}) {
  const disclosure = entry.aiDisclosure;
  if (!disclosure || !disclosure.has_synthetic_content) return null;
  const lines = disclosure.credits.map((c) => `• ${summarizeCredit(c)}`);
  const tooltip = [
    `AI disclosure — ${disclosure.credits.length} generated clip${
      disclosure.credits.length === 1 ? "" : "s"
    }:`,
    ...lines,
  ].join("\n");
  return (
    <span
      title={tooltip}
      aria-label={`AI disclosure: ${disclosure.credits.length} generated clip${
        disclosure.credits.length === 1 ? "" : "s"
      }`}
      className="inline-flex shrink-0 items-center gap-0.5 rounded-full border border-[var(--color-warning)] bg-[rgba(245,158,11,0.12)] px-1.5 py-0.5 font-mono text-[var(--text-caption)] text-[var(--color-warning)]"
    >
      <span aria-hidden>⚠</span>
      AI
    </span>
  );
}
