/**
 * SilencesDrillDown — sparkline of silence density + list of ranges. Click
 * a row to jump the timeline cursor to that silence start. The sparkline
 * bins silence count into ~32 buckets across the active span so users
 * see distribution at a glance.
 */

import { useMemo } from "react";
import { useSilences } from "../../../state/evidenceAccessors";
import { DrillDownEmpty, formatTimecode, jumpTimelineTo } from "./DrillDownModal";

const BUCKETS = 32;

interface SilencesDrillDownProps {
  onRunIndexers?: () => void;
}

export function SilencesDrillDown({ onRunIndexers }: SilencesDrillDownProps) {
  const silences = useSilences();
  const sparkline = useMemo(() => buildSparkline(silences), [silences]);

  if (silences.length === 0) {
    return (
      <DrillDownEmpty
        message="No silences detected yet. Run the silence indexer to populate this view."
        onRunIndexers={onRunIndexers}
      />
    );
  }

  return (
    <div className="flex flex-col gap-3">
      <div
        className="flex items-end gap-[2px] h-12 px-1 py-1.5 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)]"
        role="img"
        aria-label="Silence density sparkline across timeline"
      >
        {sparkline.map((value, i) => (
          <span
            key={i}
            className="flex-1 bg-[var(--color-text-secondary)] rounded-[1px] opacity-80"
            style={{ height: `${Math.max(6, value * 100)}%` }}
          />
        ))}
      </div>
      <div
        className="flex flex-col gap-1"
        role="list"
        aria-label={`${silences.length} silence ranges`}
      >
        {silences.map((silence, idx) => (
          <button
            key={`${silence.start_s}-${idx}`}
            type="button"
            role="listitem"
            onClick={() => jumpTimelineTo(silence.start_s)}
            className="flex items-center justify-between gap-3 px-2 py-1.5 rounded-[var(--radius-sm)] bg-[var(--color-surface-card)] hover:bg-[var(--color-surface-card-hover)] text-left"
            title={`Jump to ${formatTimecode(silence.start_s)}`}
          >
            <span className="font-mono text-[var(--text-caption)] tabular-nums text-[var(--color-text-secondary)]">
              {formatTimecode(silence.start_s)} → {formatTimecode(silence.end_s)}
            </span>
            <span className="font-mono text-[var(--text-caption)] tabular-nums text-[var(--color-text-muted)]">
              {silence.duration_s.toFixed(2)}s
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

function buildSparkline(
  silences: ReadonlyArray<{ start_s: number; end_s: number }>,
): number[] {
  if (silences.length === 0) return new Array(BUCKETS).fill(0);
  const max = silences[silences.length - 1]!.end_s;
  const span = Math.max(0.001, max);
  const buckets = new Array(BUCKETS).fill(0);
  for (const s of silences) {
    const idx = Math.min(BUCKETS - 1, Math.floor((s.start_s / span) * BUCKETS));
    buckets[idx]! += 1;
  }
  const peak = Math.max(...buckets);
  return peak > 0 ? buckets.map((v) => v / peak) : buckets;
}
