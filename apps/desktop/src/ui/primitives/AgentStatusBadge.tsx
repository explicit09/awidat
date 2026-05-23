import { cn } from "../cn";

export type AgentStatus =
  | "online"
  | "analyzing"
  | "generating"
  | "awaiting-review"
  | "idle"
  | "offline";

const META: Record<AgentStatus, { color: string; label: string }> = {
  online: { color: "var(--color-success)", label: "Agent online" },
  analyzing: { color: "var(--color-processing)", label: "Analyzing media" },
  generating: { color: "var(--color-brand-purple)", label: "Generating proposals" },
  "awaiting-review": { color: "var(--color-warning)", label: "Awaiting review" },
  idle: { color: "var(--color-text-muted)", label: "Idle" },
  offline: { color: "var(--color-danger)", label: "Agent offline" },
};

export type AgentStatusBadgeProps = {
  status: AgentStatus;
  detail?: string;
  className?: string;
  pulse?: boolean;
};

export function AgentStatusBadge({ status, detail, className, pulse = true }: AgentStatusBadgeProps) {
  const meta = META[status];
  const isPulsing = pulse && (status === "analyzing" || status === "generating" || status === "awaiting-review");
  return (
    <div
      className={cn(
        "inline-flex items-center gap-2",
        "max-w-full shrink-0 overflow-hidden whitespace-nowrap",
        "h-7 px-2",
        className,
      )}
    >
      <span className="relative h-2 w-2">
        <span
          className={cn(
            "absolute inset-0 rounded-full",
            isPulsing && "animate-ping opacity-60",
          )}
          style={{ backgroundColor: meta.color }}
          aria-hidden
        />
        <span className="absolute inset-0 rounded-full" style={{ backgroundColor: meta.color }} />
      </span>
      <span className="shrink-0 text-[var(--text-body-sm)] font-medium text-[var(--color-text-primary)]">
        {meta.label}
      </span>
      {detail ? (
        <span className="min-w-0 truncate text-[var(--text-caption)] text-[var(--color-text-muted)]">· {detail}</span>
      ) : null}
    </div>
  );
}
