import { cn } from "../cn";
import { confidenceLevel, type ConfidenceLevel } from "./ConfidenceMeter";

const COLOR: Record<ConfidenceLevel, string> = {
  high: "var(--color-success)",
  medium: "var(--color-warning)",
  low: "var(--color-risk)",
  "very-low": "var(--color-danger)",
};

const LABEL: Record<ConfidenceLevel, string> = {
  high: "High",
  medium: "Medium",
  low: "Low",
  "very-low": "Very low",
};

export type ConfidenceRingProps = {
  /** 0..1 */
  score: number;
  size?: number;
  thickness?: number;
  showLabel?: boolean;
  className?: string;
};

export function ConfidenceRing({
  score,
  size = 64,
  thickness = 6,
  showLabel = true,
  className,
}: ConfidenceRingProps) {
  const clamped = Math.max(0, Math.min(1, score));
  const level = confidenceLevel(clamped);
  const color = COLOR[level];
  const pct = Math.round(clamped * 100);
  const radius = (size - thickness) / 2;
  const circumference = 2 * Math.PI * radius;
  const dashOffset = circumference * (1 - clamped);
  return (
    <div
      className={cn("inline-flex flex-col items-center gap-1", className)}
      aria-label={`Confidence ${pct}%`}
      role="img"
    >
      <div className="relative inline-flex items-center justify-center" style={{ width: size, height: size }}>
        <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} className="-rotate-90">
          <circle
            cx={size / 2}
            cy={size / 2}
            r={radius}
            fill="none"
            stroke="var(--color-surface-input)"
            strokeWidth={thickness}
          />
          <circle
            cx={size / 2}
            cy={size / 2}
            r={radius}
            fill="none"
            stroke={color}
            strokeWidth={thickness}
            strokeLinecap="round"
            strokeDasharray={circumference}
            strokeDashoffset={dashOffset}
            style={{
              transition: "stroke-dashoffset 400ms cubic-bezier(0.16, 1, 0.3, 1)",
            }}
          />
        </svg>
        <span
          className="absolute inset-0 flex flex-col items-center justify-center font-mono leading-none text-[var(--color-text-primary)]"
          style={{ fontSize: size * 0.28 }}
        >
          {pct}
        </span>
      </div>
      {showLabel ? (
        <span
          className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold"
          style={{ color }}
        >
          {LABEL[level]}
        </span>
      ) : null}
    </div>
  );
}
