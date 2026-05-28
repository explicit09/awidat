import type { HTMLAttributes } from "react";
import { cva } from "class-variance-authority";
import { cn } from "../cn";
import {
  resolveStatusLabel,
  type JobPillState,
  type ProposalPillState,
  type StatusPillProps,
} from "./StatusPill.ts";

// Re-export the public contract from the React entry point so consumers
// can `import { StatusPill, resolveStatusLabel, ... } from ".../StatusPill"`.
export { resolveStatusLabel };
export type { JobPillState, ProposalPillState, StatusPillProps };

const pill = cva(
  [
    "inline-flex items-center gap-1.5",
    "rounded-full border",
    "font-semibold",
    "whitespace-nowrap",
  ],
  {
    variants: {
      family: { job: "", proposal: "" },
      state: {
        idle: "",
        running: "",
        ready: "",
        failed: "",
        proposed: "",
        accepted: "",
        rejected: "",
        revised: "",
      },
      size: {
        sm: "h-4 px-1.5 text-[10px] leading-none",
        md: "h-[18px] px-2 text-[11px] leading-none",
      },
    },
    compoundVariants: [
      { family: "job", state: "idle", className: "bg-[var(--color-job-idle-fill)] border-[var(--color-job-idle-border)] text-[var(--color-job-idle-text)]" },
      { family: "job", state: "running", className: "bg-[var(--color-job-running-fill)] border-[var(--color-job-running-border)] text-[var(--color-job-running-text)]" },
      { family: "job", state: "ready", className: "bg-[var(--color-job-ready-fill)] border-[var(--color-job-ready-border)] text-[var(--color-job-ready-text)]" },
      { family: "job", state: "failed", className: "bg-[var(--color-job-failed-fill)] border-[var(--color-job-failed-border)] text-[var(--color-job-failed-text)]" },
      { family: "proposal", state: "proposed", className: "bg-[var(--color-proposal-proposed-fill)] border-[var(--color-proposal-proposed-border)] text-[var(--color-proposal-proposed-text)]" },
      { family: "proposal", state: "accepted", className: "bg-[var(--color-proposal-accepted-fill)] border-[var(--color-proposal-accepted-border)] text-[var(--color-proposal-accepted-text)]" },
      { family: "proposal", state: "rejected", className: "bg-[var(--color-proposal-rejected-fill)] border-[var(--color-proposal-rejected-border)] text-[var(--color-proposal-rejected-text)]" },
      { family: "proposal", state: "revised", className: "bg-[var(--color-proposal-revised-fill)] border-[var(--color-proposal-revised-border)] text-[var(--color-proposal-revised-text)]" },
    ],
    defaultVariants: { size: "md", family: "job", state: "idle" },
  },
);

const dot = cva("h-1.5 w-1.5 shrink-0 rounded-full", {
  variants: {
    family: { job: "", proposal: "" },
    state: {
      idle: "", running: "", ready: "", failed: "",
      proposed: "", accepted: "", rejected: "", revised: "",
    },
  },
  compoundVariants: [
    { family: "job", state: "idle", className: "bg-[var(--color-job-idle-dot)]" },
    { family: "job", state: "running", className: "bg-[var(--color-job-running-dot)] shadow-[0_0_6px_rgba(255,122,24,0.6)]" },
    { family: "job", state: "ready", className: "bg-[var(--color-job-ready-dot)]" },
    { family: "job", state: "failed", className: "bg-[var(--color-job-failed-dot)]" },
    { family: "proposal", state: "proposed", className: "bg-[var(--color-proposal-proposed-dot)]" },
    { family: "proposal", state: "accepted", className: "bg-[var(--color-proposal-accepted-dot)]" },
    { family: "proposal", state: "rejected", className: "bg-[var(--color-proposal-rejected-dot)]" },
    { family: "proposal", state: "revised", className: "bg-[var(--color-proposal-revised-dot)]" },
  ],
});

export function StatusPill(props: StatusPillProps & HTMLAttributes<HTMLSpanElement>) {
  const { family, state, label, dotOnly = false, size = "md", className, ...rest } = props;
  const percent = "percent" in props ? props.percent : undefined;
  const text = resolveStatusLabel({ family, state, label, percent });

  if (dotOnly) {
    return <span className={cn(dot({ family, state }), className)} aria-label={text} {...rest} />;
  }
  return (
    <span className={cn(pill({ family, state, size }), className)} {...rest}>
      <span className={dot({ family, state })} aria-hidden />
      {text}
    </span>
  );
}
