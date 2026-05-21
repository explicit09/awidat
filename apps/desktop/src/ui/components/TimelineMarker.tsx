import { Check, Info, TriangleAlert, X, MoreHorizontal } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "../cn";

export type TimelineMarkerKind =
  | "accepted"
  | "rejected"
  | "pending"
  | "warning"
  | "info"
  | "skipped"
  | "selected";

const META: Record<
  TimelineMarkerKind,
  { color: string; icon: ReactNode; aria: string }
> = {
  accepted: { color: "var(--color-success)", icon: <Check />, aria: "Accepted" },
  rejected: { color: "var(--color-danger)", icon: <X />, aria: "Rejected" },
  pending: { color: "var(--color-warning)", icon: null, aria: "Pending" },
  warning: { color: "var(--color-warning)", icon: <TriangleAlert />, aria: "Warning" },
  info: { color: "var(--color-info)", icon: <Info />, aria: "Info" },
  skipped: { color: "var(--color-text-muted)", icon: <MoreHorizontal />, aria: "Skipped" },
  selected: { color: "var(--color-brand-purple)", icon: null, aria: "Selected" },
};

export type TimelineMarkerProps = {
  kind: TimelineMarkerKind;
  label?: string;
  className?: string;
  onClick?: () => void;
};

export function TimelineMarker({ kind, label, className, onClick }: TimelineMarkerProps) {
  const meta = META[kind];
  if (kind === "pending") {
    return (
      <button
        type="button"
        onClick={onClick}
        aria-label={label ?? meta.aria}
        className={cn(
          "h-6 w-1.5 rounded-full",
          "transition-transform duration-[120ms]",
          "hover:scale-110 focus-visible:outline-2 focus-visible:outline-[var(--color-border-focus)]",
          className,
        )}
        style={{ backgroundColor: meta.color }}
      />
    );
  }
  if (kind === "selected") {
    return (
      <div
        aria-label={label ?? meta.aria}
        className={cn("h-full w-0.5", className)}
        style={{ backgroundColor: meta.color, boxShadow: `0 0 12px ${meta.color}` }}
      />
    );
  }
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label ?? meta.aria}
      className={cn(
        "inline-flex h-5 w-5 items-center justify-center rounded-full",
        "text-white [&_svg]:h-3 [&_svg]:w-3 [&_svg]:stroke-[2.5]",
        "transition-transform duration-[120ms] hover:scale-110",
        "focus-visible:outline-2 focus-visible:outline-[var(--color-border-focus)]",
        className,
      )}
      style={{ backgroundColor: meta.color }}
    >
      {meta.icon}
    </button>
  );
}
