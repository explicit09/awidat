import { cn } from "../cn";

/**
 * ChannelLanes — the colored signal lanes from concept Screen 4.
 *
 * Renders one or more named lanes (sentiment / energy / confidence /
 * speaker) as horizontal colored strips aligned to a shared timeline.
 * Each lane carries an array of segments with a value (0..1) or a
 * categorical key that maps to a color.
 *
 * The component is data-agnostic — it just draws what it's given. Real
 * computation (sentiment via NLP, energy via RMS, etc.) belongs to
 * indexing-pipeline work that lands in Phase 4.
 */

export type LaneSegment = {
  startS: number;
  endS: number;
  /** 0..1 — used by gradient lanes (energy, confidence). */
  value?: number;
  /** Categorical key — used by named lanes (speaker, sentiment). */
  key?: string;
  /** Optional override; takes precedence over value/key. */
  color?: string;
};

export type ChannelLane = {
  id: string;
  label: string;
  /** When set, override the per-segment color resolution. */
  paletteKey?: "sentiment" | "energy" | "confidence" | "speaker";
  segments: LaneSegment[];
};

export type ChannelLanesProps = {
  durationS: number;
  lanes: ChannelLane[];
  /** Optional global playhead in seconds. */
  currentTimeS?: number;
  className?: string;
};

const SENTIMENT_COLORS: Record<string, string> = {
  positive: "var(--color-success)",
  neutral: "var(--color-text-muted)",
  negative: "var(--color-danger)",
};

// brand-sweep: speaker C uses brand orange as a categorical palette slot
// (decorative, not a CTA/status), alongside the viz speaker hues and purple.
const SPEAKER_COLORS = [
  "var(--color-viz-speaker-a)",
  "var(--color-viz-speaker-b)",
  "var(--color-brand)",
  "var(--color-brand-purple)",
];

function energyGradient(value: number): string {
  // Yellow → orange → red as value increases.
  if (value < 0.33) return "var(--color-success)";
  if (value < 0.66) return "var(--color-warning)";
  return "var(--color-risk)";
}

function confidenceGradient(value: number): string {
  if (value >= 0.8) return "var(--color-success)";
  if (value >= 0.55) return "var(--color-warning)";
  if (value >= 0.3) return "var(--color-risk)";
  return "var(--color-danger)";
}

function colorFor(lane: ChannelLane, seg: LaneSegment): string {
  if (seg.color) return seg.color;
  switch (lane.paletteKey) {
    case "sentiment":
      return SENTIMENT_COLORS[seg.key ?? "neutral"] ?? "var(--color-text-muted)";
    case "speaker": {
      const idx = Number.parseInt(seg.key ?? "0", 10) || 0;
      return SPEAKER_COLORS[idx % SPEAKER_COLORS.length];
    }
    case "energy":
      return energyGradient(seg.value ?? 0);
    case "confidence":
      return confidenceGradient(seg.value ?? 0);
    default:
      return "var(--color-text-muted)";
  }
}

export function ChannelLanes({ durationS, lanes, currentTimeS, className }: ChannelLanesProps) {
  const safe = durationS > 0 ? durationS : 1;
  const playPct = currentTimeS !== undefined
    ? Math.max(0, Math.min(100, (currentTimeS / safe) * 100))
    : null;
  return (
    <div className={cn("relative", className)}>
      <div className="flex flex-col gap-1">
        {lanes.map((lane) => (
          <div key={lane.id} className="grid grid-cols-[88px_1fr] items-center gap-3">
            <div className="text-[var(--text-micro)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)] text-right">
              {lane.label}
            </div>
            <div className="relative h-3 rounded-[var(--radius-xs)] bg-[var(--color-surface-input)] overflow-hidden">
              {lane.segments.map((seg, i) => {
                const left = (seg.startS / safe) * 100;
                const width = Math.max(0.4, ((seg.endS - seg.startS) / safe) * 100);
                const color = colorFor(lane, seg);
                return (
                  <div
                    key={`${lane.id}-${i}`}
                    className="absolute inset-y-0"
                    style={{
                      left: `${left}%`,
                      width: `${width}%`,
                      backgroundColor: color,
                      opacity: 0.85,
                    }}
                    title={lane.label}
                  />
                );
              })}
            </div>
          </div>
        ))}
      </div>
      {playPct !== null ? (
        <div
          aria-hidden
          className="pointer-events-none absolute top-0 bottom-0 w-px bg-[var(--color-viz-playhead)]"
          style={{ left: `calc(88px + 12px + ${playPct}% * (1 - (88px + 12px) / 100%))` }}
        />
      ) : null}
    </div>
  );
}
