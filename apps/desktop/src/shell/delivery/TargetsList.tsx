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
 * Publishing-provider keys that map 1:1 to a delivery target. Used to
 * decide whether the "Upload after render?" toggle is shown on a
 * target card — captions / cover / custom are local exports so they
 * don't have an auto-publish option.
 */
const UPLOAD_CAPABLE_TARGETS: ReadonlySet<DeliveryTargetKey> = new Set([
  "youtube",
  "tiktok",
  "instagram",
]);

/** `true` if this target maps to a publishing provider. */
export function isUploadCapableTarget(key: DeliveryTargetKey): boolean {
  return UPLOAD_CAPABLE_TARGETS.has(key);
}

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
  uploadAfterRender,
  onToggleTarget,
  onToggleUploadAfterRender,
  onAgentRepair,
}: {
  resolvedTargets: DeliveryTarget[];
  findings: PreflightFinding[];
  runningByTarget: Partial<Record<DeliveryTargetKey, number>>;
  /**
   * Provider keys the user has opted into for auto-upload-after-render.
   * Each capable target card flips between Off and On based on this set.
   * Defaults to empty (privacy/cost safety — opt in explicitly).
   */
  uploadAfterRender?: ReadonlySet<DeliveryTargetKey>;
  onToggleTarget?: (key: DeliveryTargetKey) => void;
  /** Flip the auto-upload toggle for one target. */
  onToggleUploadAfterRender?: (key: DeliveryTargetKey) => void;
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
                uploadAfterRender={uploadAfterRender?.has(target.key) ?? false}
                onClick={() => onToggleTarget?.(target.key)}
                onToggleUpload={
                  isUploadCapableTarget(target.key) && onToggleUploadAfterRender
                    ? () => onToggleUploadAfterRender(target.key)
                    : undefined
                }
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
 *  for this target replaces the dot with a StatusPill (running).
 *
 *  When the target maps to a publishing provider, a small
 *  "Upload after render?" pip appears under the spec line — flips
 *  between an inactive ring and a brand-filled pill. Hidden for
 *  non-publishable targets (captions / cover / custom).
 *
 *  The pip click is `stopPropagation`'d so toggling auto-upload
 *  doesn't also flip the target-active state. */
function TargetCard({
  target,
  runningPercent,
  uploadAfterRender,
  onClick,
  onToggleUpload,
}: {
  target: DeliveryTarget;
  runningPercent?: number;
  uploadAfterRender: boolean;
  onClick: () => void;
  onToggleUpload?: () => void;
}) {
  const meta = TARGET_META[target.key];
  const Icon = meta.icon;
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onClick}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onClick();
        }
      }}
      aria-pressed={target.active}
      title={target.spec ?? meta.spec}
      className={cn(
        "group flex items-center gap-2.5 rounded-[var(--radius-md)] px-2.5 py-2 text-left",
        "border transition-colors cursor-pointer",
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
        {onToggleUpload ? (
          <button
            type="button"
            onClick={(e) => {
              // Don't propagate up to the card's onClick — that flips
              // the target-active state, which is independent of the
              // auto-upload opt-in.
              e.stopPropagation();
              onToggleUpload();
            }}
            aria-pressed={uploadAfterRender}
            title={
              uploadAfterRender
                ? "Auto-upload after render is ON"
                : "Auto-upload after render is OFF"
            }
            className={cn(
              "mt-1 inline-flex items-center gap-1 rounded-full border px-1.5 py-0.5 text-[var(--text-caption)] transition-colors",
              uploadAfterRender
                ? "border-[var(--color-brand)] bg-[var(--color-brand)]/15 text-[var(--color-brand)]"
                : "border-[var(--color-border)] text-[var(--color-text-muted)] hover:border-[var(--color-text-muted)]",
            )}
          >
            <span
              aria-hidden
              className={cn(
                "h-1.5 w-1.5 rounded-full",
                uploadAfterRender
                  ? "bg-[var(--color-brand)]"
                  : "bg-[var(--color-text-muted)]",
              )}
            />
            Upload {uploadAfterRender ? "on" : "off"}
          </button>
        ) : null}
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
    </div>
  );
}
