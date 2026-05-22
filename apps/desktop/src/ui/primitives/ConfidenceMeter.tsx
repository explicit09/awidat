import { cn } from "../cn";

export type ConfidenceLevel = "high" | "medium" | "low" | "very-low";

export function confidenceLevel(score: number): ConfidenceLevel {
  if (score >= 0.8) return "high";
  if (score >= 0.55) return "medium";
  if (score >= 0.3) return "low";
  return "very-low";
}

const LABEL: Record<ConfidenceLevel, string> = {
  high: "High",
  medium: "Medium",
  low: "Low",
  "very-low": "Very low",
};

const COLOR: Record<ConfidenceLevel, string> = {
  high: "var(--color-success)",
  medium: "var(--color-warning)",
  low: "var(--color-risk)",
  "very-low": "var(--color-danger)",
};

export type ConfidenceMeterProps = {
  /** 0..1 */
  score: number;
  label?: string;
  size?: "sm" | "md";
  className?: string;
};

export function ConfidenceMeter({ score, label, size = "md", className }: ConfidenceMeterProps) {
  const clamped = Math.max(0, Math.min(1, score));
  const level = confidenceLevel(clamped);
  const color = COLOR[level];
  const pct = Math.round(clamped * 100);
  return (
    <div className={cn("flex flex-col gap-1.5", className)} aria-label={`Confidence ${pct}%`}>
      <div className="flex items-baseline justify-between">
        <span className="text-[var(--text-label)] font-semibold tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-secondary)] uppercase">
          {label ?? "Confidence"}
        </span>
        <span className="font-mono text-[var(--text-body-sm)] text-[var(--color-text-primary)]">
          {pct}
          <span className="ml-1 text-[var(--color-text-muted)]">{LABEL[level]}</span>
        </span>
      </div>
      <div
        className={cn(
          "w-full rounded-full overflow-hidden bg-[var(--color-surface-input)]",
          size === "sm" ? "h-1" : "h-1.5",
        )}
        role="progressbar"
        aria-valuenow={pct}
        aria-valuemin={0}
        aria-valuemax={100}
      >
        <div
          className="h-full rounded-full transition-[width] duration-[400ms] ease-[cubic-bezier(0.16,1,0.3,1)]"
          style={{ width: `${pct}%`, backgroundColor: color }}
        />
      </div>
    </div>
  );
}
