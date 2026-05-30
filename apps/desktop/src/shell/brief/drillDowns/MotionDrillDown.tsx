/**
 * MotionDrillDown — heat-strip visualization where each cell is one
 * motion-magnitude sample. Intensity drives both height and opacity so
 * peaks read against quiet stretches without separate axes. Click a
 * cell to jump the timeline cursor to that window.
 */

import { useMotionRegions } from "../../../state/evidenceAccessors";
import { DrillDownEmpty, formatTimecode, jumpTimelineTo } from "./DrillDownModal";

interface MotionDrillDownProps {
  onRunIndexers?: () => void;
}

export function MotionDrillDown({ onRunIndexers }: MotionDrillDownProps) {
  const regions = useMotionRegions();

  if (regions.length === 0) {
    return (
      <DrillDownEmpty
        message="No motion data yet. Run the motion indexer to populate this view."
        onRunIndexers={onRunIndexers}
      />
    );
  }

  const totalDuration = regions[regions.length - 1]!.end_s;
  const peakIntensity = Math.max(...regions.map((r) => r.intensity));

  return (
    <div className="flex flex-col gap-3">
      <div
        className="flex items-end gap-[1px] h-24 px-1 py-1.5 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)]"
        role="list"
        aria-label="Motion heat strip"
      >
        {regions.map((r) => (
          <button
            key={r.id}
            type="button"
            role="listitem"
            onClick={() => jumpTimelineTo(r.start_s)}
            title={`${formatTimecode(r.start_s)} · intensity ${r.intensity.toFixed(2)}`}
            className="flex-1 rounded-[1px]"
            style={{
              height: `${Math.max(6, r.intensity * 100)}%`,
              opacity: 0.35 + r.intensity * 0.65,
              backgroundColor: "var(--color-text-secondary)",
            }}
          />
        ))}
      </div>
      <div className="flex justify-between font-mono text-[var(--text-caption)] tabular-nums text-[var(--color-text-muted)]">
        <span>{formatTimecode(0)}</span>
        <span>
          {regions.length} windows · peak {peakIntensity.toFixed(2)}
        </span>
        <span>{formatTimecode(totalDuration)}</span>
      </div>
    </div>
  );
}
