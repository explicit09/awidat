/**
 * Pure logic + type contract for the StatusPill primitive.
 *
 * Lives in `.ts` (no JSX) so that Node's `--experimental-strip-types` loader
 * can import `resolveStatusLabel` directly from tests without needing a JSX
 * transformer. The React component lives next to it in `StatusPill.tsx` and
 * re-exports everything here.
 */

export type JobState = "idle" | "running" | "ready" | "failed";
export type ProposalState = "proposed" | "accepted" | "rejected" | "revised";

/**
 * Discriminated union so `percent` is only valid on `family: 'job', state: 'running'`.
 * TypeScript enforces this at every call site; runtime double-checks (see resolveStatusLabel).
 */
export type StatusPillProps =
  | {
      family: "job";
      state: "running";
      label?: string;
      percent?: number;
      dotOnly?: boolean;
      size?: "sm" | "md";
    }
  | {
      family: "job";
      state: Exclude<JobState, "running">;
      label?: string;
      percent?: never;
      dotOnly?: boolean;
      size?: "sm" | "md";
    }
  | {
      family: "proposal";
      state: ProposalState;
      label?: string;
      percent?: never;
      dotOnly?: boolean;
      size?: "sm" | "md";
    };

export const DEFAULT_LABELS: Record<"job" | "proposal", Record<string, string>> = {
  job: { idle: "Idle", running: "Running", ready: "Ready", failed: "Failed" },
  proposal: { proposed: "Proposed", accepted: "Accepted", rejected: "Rejected", revised: "Revised" },
};

/** Pure label/percent resolution — testable without rendering. */
export function resolveStatusLabel(
  opts: { family: "job" | "proposal"; state: string; label?: string; percent?: number },
): string {
  const base = opts.label ?? DEFAULT_LABELS[opts.family]?.[opts.state] ?? opts.state;
  if (opts.state !== "running" || opts.percent === undefined) return base;
  const clamped = Math.max(0, Math.min(100, Math.round(opts.percent)));
  return `${base} · ${clamped}%`;
}
