import type { HTMLAttributes, ReactNode } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "../cn";

export const PILL_STATUSES = [
  "proposed",
  "pending",
  "accepted",
  "rejected",
  "reviewing",
  "processing",
  "ready",
  "failed",
  "warning",
  "missing",
  "revised",
  "neutral",
] as const;

export type PillStatus = (typeof PILL_STATUSES)[number];

const pill = cva(
  [
    "inline-flex items-center gap-1.5",
    "h-[var(--layout-pill-h)] px-2",
    "rounded-[var(--radius-sm)] border",
    "font-semibold",
    "text-[var(--text-label)] leading-[1] tracking-[var(--text-label--letter-spacing)]",
    "whitespace-nowrap",
  ],
  {
    variants: {
      status: {
        proposed:
          "bg-[var(--color-pill-proposed-fill)] border-[var(--color-pill-proposed-border)] text-[var(--color-pill-proposed-text)]",
        pending:
          "bg-[var(--color-pill-pending-fill)] border-[var(--color-pill-pending-border)] text-[var(--color-pill-pending-text)]",
        accepted:
          "bg-[var(--color-pill-accepted-fill)] border-[var(--color-pill-accepted-border)] text-[var(--color-pill-accepted-text)]",
        rejected:
          "bg-[var(--color-pill-rejected-fill)] border-[var(--color-pill-rejected-border)] text-[var(--color-pill-rejected-text)]",
        reviewing:
          "bg-[var(--color-pill-reviewing-fill)] border-[var(--color-pill-reviewing-border)] text-[var(--color-pill-reviewing-text)]",
        processing:
          "bg-[var(--color-pill-processing-fill)] border-[var(--color-pill-processing-border)] text-[var(--color-pill-processing-text)]",
        ready:
          "bg-[var(--color-pill-ready-fill)] border-[var(--color-pill-ready-border)] text-[var(--color-pill-ready-text)]",
        failed:
          "bg-[var(--color-pill-failed-fill)] border-[var(--color-pill-failed-border)] text-[var(--color-pill-failed-text)]",
        warning:
          "bg-[var(--color-pill-warning-fill)] border-[var(--color-pill-warning-border)] text-[var(--color-pill-warning-text)]",
        missing:
          "bg-[var(--color-pill-missing-fill)] border-[var(--color-pill-missing-border)] text-[var(--color-pill-missing-text)]",
        revised:
          "bg-[var(--color-pill-revised-fill)] border-[var(--color-pill-revised-border)] text-[var(--color-pill-revised-text)]",
        neutral:
          "bg-[var(--color-surface-card)] border-[var(--color-border)] text-[var(--color-text-secondary)]",
      },
      dot: {
        true: "",
        false: "",
      },
    },
    defaultVariants: { status: "neutral", dot: true },
  },
);

const dot = cva("h-1.5 w-1.5 rounded-full", {
  variants: {
    status: {
      proposed: "bg-[var(--color-pill-proposed-dot)]",
      pending: "bg-[var(--color-pill-pending-dot)]",
      accepted: "bg-[var(--color-pill-accepted-dot)]",
      rejected: "bg-[var(--color-pill-rejected-dot)]",
      reviewing: "bg-[var(--color-pill-reviewing-dot)]",
      processing: "bg-[var(--color-pill-processing-dot)]",
      ready: "bg-[var(--color-pill-ready-dot)]",
      failed: "bg-[var(--color-pill-failed-dot)]",
      warning: "bg-[var(--color-pill-warning-dot)]",
      missing: "bg-[var(--color-pill-missing-dot)]",
      revised: "bg-[var(--color-pill-revised-dot)]",
      neutral: "bg-[var(--color-text-muted)]",
    },
  },
});

export type PillProps = HTMLAttributes<HTMLSpanElement> &
  VariantProps<typeof pill> & {
    children: ReactNode;
  };

export function Pill({ status = "neutral", dot: showDot = true, className, children, ...rest }: PillProps) {
  return (
    <span className={cn(pill({ status, dot: showDot }), className)} {...rest}>
      {showDot ? <span className={dot({ status })} aria-hidden /> : null}
      {children}
    </span>
  );
}
