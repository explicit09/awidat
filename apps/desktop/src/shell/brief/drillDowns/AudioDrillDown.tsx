/**
 * AudioDrillDown — sparkline of RMS energy across the timeline. Bins the
 * raw audio samples into ~120 buckets so the modal renders in O(buckets)
 * regardless of source length.
 */

import { useMemo } from "react";
import { useAudioSamples } from "../../../state/evidenceAccessors";
import { DrillDownEmpty, formatTimecode, jumpTimelineTo } from "./DrillDownModal";

const BUCKETS = 120;

interface AudioDrillDownProps {
  onRunIndexers?: () => void;
}

export function AudioDrillDown({ onRunIndexers }: AudioDrillDownProps) {
  const samples = useAudioSamples();
  const sparkline = useMemo(() => buildSparkline(samples), [samples]);

  if (samples.length === 0) {
    return (
      <DrillDownEmpty
        message="No audio energy data yet. Run the audio-energy indexer to populate this view."
        onRunIndexers={onRunIndexers}
      />
    );
  }

  const lastSample = samples[samples.length - 1]!;
  const duration = lastSample.at_s;

  return (
    <div className="flex flex-col gap-3">
      <button
        type="button"
        onClick={(event) => {
          const rect = event.currentTarget.getBoundingClientRect();
          const x = event.clientX - rect.left;
          const fraction = Math.max(0, Math.min(1, x / rect.width));
          jumpTimelineTo(fraction * duration);
        }}
        className="flex items-end gap-[1px] h-20 px-1 py-1.5 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] hover:border-[var(--color-border)] transition-colors"
        aria-label="Audio energy sparkline — click to jump"
      >
        {sparkline.map((value, i) => (
          <span
            key={i}
            className="flex-1 bg-[var(--color-text-secondary)] rounded-[1px]"
            style={{
              height: `${Math.max(4, value * 100)}%`,
              opacity: 0.5 + value * 0.5,
            }}
          />
        ))}
      </button>
      <div className="flex justify-between font-mono text-[var(--text-caption)] tabular-nums text-[var(--color-text-muted)]">
        <span>{formatTimecode(0)}</span>
        <span>
          {samples.length} samples · peak {Math.max(...samples.map((s) => s.rms)).toFixed(2)}
        </span>
        <span>{formatTimecode(duration)}</span>
      </div>
    </div>
  );
}

function buildSparkline(samples: ReadonlyArray<{ at_s: number; rms: number }>): number[] {
  if (samples.length === 0) return new Array(BUCKETS).fill(0);
  const max = samples[samples.length - 1]!.at_s;
  const span = Math.max(0.001, max);
  const sums = new Array(BUCKETS).fill(0);
  const counts = new Array(BUCKETS).fill(0);
  for (const s of samples) {
    const idx = Math.min(BUCKETS - 1, Math.floor((s.at_s / span) * BUCKETS));
    sums[idx]! += s.rms;
    counts[idx]! += 1;
  }
  return sums.map((sum, i) => (counts[i]! > 0 ? sum / counts[i]! : 0));
}
