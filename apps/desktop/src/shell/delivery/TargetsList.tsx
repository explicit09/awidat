import { ShieldCheck } from "lucide-react";
import {
  BrandIcon,
  Inline,
  Stack,
  StatusPill,
  cn,
} from "../../ui";
import { TARGET_META } from "./targetMeta";
import {
  countBySeverity,
  type DeliveryTarget,
  type DeliveryTargetKey,
  type PreflightFinding,
} from "./types";

/**
 * Left-rail target picker. Each target is a selectable card; the
 * selected ones get --color-surface-selected + a small brand dot.
 * The format spec sits on a second line and also doubles as the
 * native `title` so it stays visible as a tooltip on hover. If a
 * target is currently rendering, a `<StatusPill family="job"
 * state="running" />` replaces the dot with a percent badge.
 *
 * Also includes a small Repair affordance underneath that surfaces
 * the first blocker preflight finding so the user has a one-click
 * jump from "I picked my targets" to "ok, agent: fix this".
 */
export function TargetsList({
  resolvedTargets,
  findings,
  runningByTarget,
  onToggleTarget,
  onAgentRepair,
}: {
  resolvedTargets: DeliveryTarget[];
  findings: PreflightFinding[];
  runningByTarget: Partial<Record<DeliveryTargetKey, number>>;
  onToggleTarget?: (key: DeliveryTargetKey) => void;
  onAgentRepair?: (finding: PreflightFinding) => void;
}) {
  const counts = countBySeverity(findings);
  const blocker = findings.find(
    (f) =>
      f.severity === "error" ||
      f.severity === "failure" ||
      f.severity === "warning",
  );
  const selectedCount = resolvedTargets.filter((t) => t.active).length;
  return (
    <div className="flex flex-col min-h-0 p-3 gap-4 overflow-y-auto">
      <div>
        <Inline justify="between" align="baseline" className="mb-2">
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
            Targets
          </span>
          <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
            {selectedCount}/{resolvedTargets.length}
          </span>
        </Inline>
        <Stack gap="1">
          {resolvedTargets.map((target) => {
            const percent = runningByTarget[target.key];
            return (
              <TargetCard
                key={target.key}
                target={target}
                runningPercent={percent}
                onClick={() => onToggleTarget?.(target.key)}
              />
            );
          })}
        </Stack>
      </div>
      {blocker ? (
        <div>
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
            Repair
          </span>
          <button
            type="button"
            onClick={() => onAgentRepair?.(blocker)}
            className="mt-2 w-full rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2.5 text-left transition-colors hover:border-[var(--color-text-muted)] hover:bg-[var(--color-surface-card-hover)]"
          >
            <Inline gap="2" align="center" className="mb-1">
              <ShieldCheck className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-warning)]" />
              <span className="text-[var(--text-body-sm)] font-medium text-[var(--color-text-primary)]">
                Ask agent to fix
              </span>
            </Inline>
            <p className="text-[var(--text-caption)] text-[var(--color-text-muted)] leading-snug line-clamp-3">
              {blocker.message}
            </p>
          </button>
          {counts.warning + counts.error + counts.failure > 1 ? (
            <p className="mt-2 text-[var(--text-caption)] text-[var(--color-text-muted)]">
              +{counts.warning + counts.error + counts.failure - 1} more finding
              {counts.warning + counts.error + counts.failure - 1 === 1 ? "" : "s"}
            </p>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

/** A single target card. Selected cards take the new selected
 *  surface + a brand dot in the checkmark slot. A running render
 *  for this target replaces the dot with a StatusPill (running). */
function TargetCard({
  target,
  runningPercent,
  onClick,
}: {
  target: DeliveryTarget;
  runningPercent?: number;
  onClick: () => void;
}) {
  const meta = TARGET_META[target.key];
  const Icon = meta.icon;
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={target.active}
      title={target.spec ?? meta.spec}
      className={cn(
        "group flex items-center gap-2.5 rounded-[var(--radius-md)] px-2.5 py-2 text-left",
        "border transition-colors",
        target.active
          ? "border-[var(--color-border-active)] bg-[var(--color-surface-selected)]"
          : "border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] hover:border-[var(--color-text-muted)] hover:bg-[var(--color-surface-card-hover)]",
      )}
    >
      {meta.brand ? (
        <BrandIcon
          icon={meta.brand}
          tinted={target.active}
          className={cn(
            "h-4 w-4 shrink-0",
            target.active ? "" : "opacity-60",
          )}
          style={
            target.active
              ? undefined
              : { color: "var(--color-text-muted)" }
          }
        />
      ) : (
        <Icon
          className={cn(
            "h-3.5 w-3.5 shrink-0 stroke-[1.75]",
            target.active
              ? "text-[var(--color-text-primary)]"
              : "text-[var(--color-text-muted)]",
          )}
        />
      )}
      <div className="min-w-0 flex-1">
        <p className="truncate text-[var(--text-body-sm)] font-medium text-[var(--color-text-primary)]">
          {target.label ?? meta.label}
        </p>
        <p className="truncate font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
          {target.spec ?? meta.spec}
        </p>
      </div>
      {runningPercent !== undefined ? (
        <StatusPill
          family="job"
          state="running"
          percent={runningPercent}
          size="sm"
        />
      ) : target.active ? (
        <span
          className="h-2 w-2 shrink-0 rounded-full bg-[var(--color-brand)]"
          aria-hidden
        />
      ) : (
        <span
          className="h-2 w-2 shrink-0 rounded-full border border-[var(--color-border)] group-hover:border-[var(--color-text-muted)]"
          aria-hidden
        />
      )}
    </button>
  );
}
