/**
 * ColorDrillDown — per-shot luma/saturation bars. Each row corresponds
 * to one scene in the color-analysis sidecar; bars are normalised to
 * `[0, 1]`. Click a row to jump the timeline cursor to that scene start.
 */

import { useColorStats } from "../../../state/evidenceAccessors";
import { DrillDownEmpty, formatTimecode, jumpTimelineTo } from "./DrillDownModal";

interface ColorDrillDownProps {
  onRunIndexers?: () => void;
}

export function ColorDrillDown({ onRunIndexers }: ColorDrillDownProps) {
  const stats = useColorStats();

  if (stats.length === 0) {
    return (
      <DrillDownEmpty
        message="No color stats yet. Run the color-analysis indexer to populate this view."
        onRunIndexers={onRunIndexers}
      />
    );
  }

  return (
    <div
      className="flex flex-col gap-1.5"
      role="list"
      aria-label={`${stats.length} color shots`}
    >
      {stats.map((stat) => (
        <button
          key={stat.id}
          type="button"
          role="listitem"
          onClick={() => jumpTimelineTo(stat.start_s)}
          className="flex items-center gap-3 px-2 py-1.5 rounded-[var(--radius-sm)] bg-[var(--color-surface-card)] hover:bg-[var(--color-surface-card-hover)] text-left"
          title={`Jump to ${formatTimecode(stat.start_s)}`}
        >
          <span className="font-mono text-[var(--text-caption)] tabular-nums text-[var(--color-text-secondary)] w-24 shrink-0">
            {formatTimecode(stat.start_s)} → {formatTimecode(stat.end_s)}
          </span>
          <ColorBar
            label="Luma"
            value={stat.luma_mean}
            color="rgba(255, 255, 255, 0.85)"
          />
          <ColorBar
            label="Sat"
            value={stat.saturation_mean}
            color="rgba(120, 200, 255, 0.85)"
          />
        </button>
      ))}
    </div>
  );
}

function ColorBar({
  label,
  value,
  color,
}: {
  label: string;
  value: number | undefined;
  color: string;
}) {
  const v = value ?? 0;
  return (
    <div className="flex-1 flex items-center gap-1.5 min-w-0">
      <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-caption--letter-spacing)] text-[var(--color-text-muted)] w-8 shrink-0">
        {label}
      </span>
      <div className="flex-1 h-2 rounded-[var(--radius-xs)] bg-[var(--color-surface-input)] overflow-hidden">
        <div
          className="h-full"
          style={{
            width: `${Math.max(0, Math.min(1, v)) * 100}%`,
            backgroundColor: color,
          }}
        />
      </div>
      <span className="font-mono text-[var(--text-caption)] tabular-nums text-[var(--color-text-muted)] w-10 text-right">
        {value !== undefined ? v.toFixed(2) : "—"}
      </span>
    </div>
  );
}
