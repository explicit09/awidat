import type { HTMLAttributes } from "react";
import { cva } from "class-variance-authority";
import { cn } from "../cn";
import {
  resolveStatusLabel,
  type JobPillState,
  type ProposalPillState,
  type StatusPillProps,
} from "./StatusPill.logic";

// Re-export the public contract from the React entry point so consumers
// can `import { StatusPill, resolveStatusLabel, ... } from ".../StatusPill"`.
export { resolveStatusLabel };
export type { JobPillState, ProposalPillState, StatusPillProps };

// Obsidian Glass 2026 status-pill palette. Color-coded glass chips:
// mono uppercase text, translucent semantic fill, matching ~.3-alpha
// border. State→family mapping is owned by StatusPill.logic.ts — this
// only paints the visuals for each (family, state) pair.
//
//   ready / accepted        → mint   (#5EEAD4 on rgba(45,212,191,.16))
//   proposed / revised      → amber  (#FCD34D on rgba(245,158,11,.16))
//   failed / rejected       → red    (#FCA5A5 on rgba(220,100,95,.16))
//   running                 → orange (#FF9A45 on rgba(255,122,24,.16))
//   idle                    → muted  (text-muted on rgba(255,255,255,.06))
const pill = cva(
  [
    "inline-flex items-center gap-1.5",
    "rounded-full border",
    "font-mono font-semibold uppercase tracking-[0.06em]",
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
        sm: "h-4 px-1.5 text-[9px] leading-none",
        md: "h-[18px] px-2 text-[10px] leading-none",
      },
    },
    compoundVariants: [
      { family: "job", state: "idle", className: "bg-[rgba(255,255,255,0.06)] border-[rgba(255,255,255,0.12)] text-[var(--color-text-muted)]" },
      { family: "job", state: "running", className: "bg-[rgba(255,122,24,0.16)] border-[rgba(255,122,24,0.3)] text-[#FF9A45]" },
      { family: "job", state: "ready", className: "bg-[rgba(45,212,191,0.16)] border-[rgba(45,212,191,0.3)] text-[#5EEAD4]" },
      { family: "job", state: "failed", className: "bg-[rgba(220,100,95,0.16)] border-[rgba(220,100,95,0.3)] text-[#FCA5A5]" },
      { family: "proposal", state: "proposed", className: "bg-[rgba(245,158,11,0.16)] border-[rgba(245,158,11,0.3)] text-[#FCD34D]" },
      { family: "proposal", state: "accepted", className: "bg-[rgba(45,212,191,0.16)] border-[rgba(45,212,191,0.3)] text-[#5EEAD4]" },
      { family: "proposal", state: "rejected", className: "bg-[rgba(220,100,95,0.16)] border-[rgba(220,100,95,0.3)] text-[#FCA5A5]" },
      { family: "proposal", state: "revised", className: "bg-[rgba(245,158,11,0.16)] border-[rgba(245,158,11,0.3)] text-[#FCD34D]" },
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
    { family: "job", state: "idle", className: "bg-[var(--color-text-muted)]" },
    { family: "job", state: "running", className: "bg-[#FF9A45] shadow-[0_0_6px_rgba(255,122,24,0.6)]" },
    { family: "job", state: "ready", className: "bg-[#5EEAD4]" },
    { family: "job", state: "failed", className: "bg-[#FCA5A5]" },
    { family: "proposal", state: "proposed", className: "bg-[#FCD34D]" },
    { family: "proposal", state: "accepted", className: "bg-[#5EEAD4]" },
    { family: "proposal", state: "rejected", className: "bg-[#FCA5A5]" },
    { family: "proposal", state: "revised", className: "bg-[#FCD34D]" },
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

/**
 * Discriminated mapping shape returned by call-site helpers (e.g. PILL_FOR
 * tables, `statePill` switches) that compute a (family, state) pair from
 * some upstream status string. Use with `<StatusPillFromMapping mapping={…}/>`
 * to avoid repeating the family-branching ternary at every render site.
 */
export type StatusPillMapping =
  | { family: "job"; state: JobPillState; percent?: number }
  | { family: "proposal"; state: ProposalPillState };

interface StatusPillFromMappingProps extends HTMLAttributes<HTMLSpanElement> {
  mapping: StatusPillMapping;
  label?: string;
  dotOnly?: boolean;
  size?: "sm" | "md";
}

/**
 * Renders a <StatusPill> from a `{family, state}` mapping object — useful
 * when the family/state comes from a helper that returns a discriminated
 * union. Without this, every caller has to branch on `mapping.family` to
 * satisfy StatusPillProps' discriminated union. Spreading the mapping
 * doesn't work because TS can't narrow `family` at the spread site, so the
 * branching still exists, but it lives ONCE inside this primitive instead
 * of at every call site.
 */
export function StatusPillFromMapping({
  mapping,
  label,
  dotOnly,
  size,
  className,
  ...rest
}: StatusPillFromMappingProps) {
  if (mapping.family === "job") {
    return mapping.state === "running" ? (
      <StatusPill
        {...rest}
        family="job"
        state="running"
        percent={mapping.percent}
        label={label}
        dotOnly={dotOnly}
        size={size}
        className={className}
      />
    ) : (
      <StatusPill
        {...rest}
        family="job"
        state={mapping.state}
        label={label}
        dotOnly={dotOnly}
        size={size}
        className={className}
      />
    );
  }
  return (
    <StatusPill
      {...rest}
      family="proposal"
      state={mapping.state}
      label={label}
      dotOnly={dotOnly}
      size={size}
      className={className}
    />
  );
}
