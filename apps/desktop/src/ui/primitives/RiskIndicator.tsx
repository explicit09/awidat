import { cn } from "../cn";

export type RiskLevel = "low" | "medium" | "high" | "very-high";

const LABEL: Record<RiskLevel, string> = {
  low: "Low",
  medium: "Medium",
  high: "High",
  "very-high": "Very high",
};

const DOTS: Record<RiskLevel, number> = {
  low: 1,
  medium: 2,
  high: 3,
  "very-high": 4,
};

const COLOR: Record<RiskLevel, string> = {
  low: "var(--color-success)",
  medium: "var(--color-warning)",
  high: "var(--color-risk)",
  "very-high": "var(--color-danger)",
};

export type RiskIndicatorProps = {
  level: RiskLevel;
  label?: string;
  className?: string;
};

export function RiskIndicator({ level, label, className }: RiskIndicatorProps) {
  const filled = DOTS[level];
  const color = COLOR[level];
  return (
    <div className={cn("flex items-baseline justify-between gap-3", className)}>
      <span className="text-[var(--text-label)] font-semibold tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-secondary)] uppercase">
        {label ?? "Risk"}
      </span>
      <span className="flex items-center gap-2">
        <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">{LABEL[level]}</span>
        <span className="flex items-center gap-1" aria-hidden>
          {Array.from({ length: 4 }).map((_, i) => (
            <span
              key={i}
              className="h-1.5 w-1.5 rounded-full transition-colors duration-[120ms]"
              style={{
                backgroundColor: i < filled ? color : "var(--color-border-subtle)",
              }}
            />
          ))}
        </span>
      </span>
    </div>
  );
}
