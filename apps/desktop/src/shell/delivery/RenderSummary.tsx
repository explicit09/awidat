import type { ReactNode } from "react";
import {
  Card,
  Inline,
  Stack,
  confidenceLevel,
} from "../../ui";
import type { DeliveryRenderSummary } from "./types";

/**
 * Right column — Render summary block.
 *
 * Duration / Outputs / Est. size are presented as mono KV pairs.
 * Delivery confidence is rendered as a colored bar that takes its
 * color from the confidence level (green / amber / orange / red).
 * A subtle hairline divider sits above the card so it visually
 * separates from the Issue inspector that sits above it in the
 * column.
 */
export function RenderSummary({ summary }: { summary: DeliveryRenderSummary }) {
  const score = Math.max(0, Math.min(1, summary.confidence));
  const level = confidenceLevel(score);
  const pct = Math.round(score * 100);
  const barColor =
    level === "high"
      ? "var(--color-success)"
      : level === "medium"
        ? "var(--color-warning)"
        : level === "low"
          ? "var(--color-risk)"
          : "var(--color-danger)";
  const levelLabel =
    level === "high"
      ? "High"
      : level === "medium"
        ? "Medium"
        : level === "low"
          ? "Low"
          : "Very low";
  return (
    <Stack gap="2">
      {/* divider above */}
      <div className="h-px w-full bg-[var(--color-border-subtle)]" aria-hidden />
      <Card padding="md">
        <Stack gap="3">
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
            Render summary
          </span>
          <KV label="Duration" value={summary.duration} />
          <KV
            label="Outputs"
            value={`${summary.outputs} ${summary.outputs === 1 ? "target" : "targets"}`}
          />
          {summary.estimatedSize ? (
            <KV label="Est. size" value={summary.estimatedSize} />
          ) : null}
          <Stack gap="2">
            <Inline justify="between" align="baseline">
              <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                Delivery confidence
              </span>
              <span className="font-mono text-[var(--text-body-sm)] text-[var(--color-text-primary)]">
                {pct}
                <span className="ml-1 text-[var(--color-text-muted)]">{levelLabel}</span>
              </span>
            </Inline>
            <div
              className="h-1.5 w-full overflow-hidden rounded-full bg-[var(--color-surface-input)]"
              role="progressbar"
              aria-valuenow={pct}
              aria-valuemin={0}
              aria-valuemax={100}
            >
              <div
                className="h-full rounded-full transition-[width] duration-[400ms] ease-[cubic-bezier(0.16,1,0.3,1)]"
                style={{ width: `${pct}%`, backgroundColor: barColor }}
              />
            </div>
          </Stack>
        </Stack>
      </Card>
    </Stack>
  );
}

/** Shared KV row — label uppercase muted, value mono primary. */
export function KV({ label, value }: { label: string; value: string | ReactNode }) {
  return (
    <Inline justify="between" align="baseline">
      <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        {label}
      </span>
      <span className="font-mono text-[var(--text-body-sm)] text-[var(--color-text-primary)]">{value}</span>
    </Inline>
  );
}
