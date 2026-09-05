import {
  AlertTriangle,
  CheckCircle2,
  Download,
  Save,
  Sparkles,
  Upload,
  XCircle,
} from "lucide-react";
import { useMemo, useState, type ReactNode } from "react";
import {
  BrandIcon,
  Stack,
  cn,
  type PreflightSeverity,
} from "../ui";
import { useProjectStore } from "../app/state";
import { useUploadPrefs } from "../state/uploadPrefs";
import { useRenderQueueStore } from "../app/renderQueue";
import { KV } from "./delivery/RenderSummary";
import { RenderQueuePanel } from "./delivery/RenderQueue";
import { UploadMetadataForm } from "./delivery/UploadMetadataForm";
import { CampaignApprovalPanel } from "./delivery/CampaignApprovalPanel";
import { TARGET_META, isUploadCapableTarget } from "./delivery/targetMeta";
import {
  ALL_TARGETS,
  countBySeverity,
  type DeliveryRenderSummary,
  type DeliveryTarget,
  type DeliveryTargetKey,
  type PreflightFinding,
} from "./delivery/types";

export type {
  DeliveryRenderSummary,
  DeliveryTarget,
  DeliveryTargetKey,
  PreflightFinding,
};

/** Stable scope key for the per-target metadata form *before* the
 *  user kicks an Export. The form persists draft state under this id
 *  so a reload survives; on Export, App.tsx copies the drafts onto
 *  each enqueued `RenderQueueEntry.id` so the worker can hand the
 *  per-provider metadata to the backend at upload time. */
export const DRAFT_METADATA_JOB_ID = "montage.deliver.draft";

/** Configure outputs, inspect findings, and review the render queue. */

export type DeliverySurfaceProps = {
  targets?: DeliveryTarget[];
  findings?: PreflightFinding[];
  summary?: DeliveryRenderSummary;
  onToggleTarget?: (key: DeliveryTargetKey) => void;
  onExportNow?: () => void;
  onFixIssues?: () => void;
  onSavePreset?: () => void;
  onGenerateVariants?: () => void;
};


export function DeliverySurface({
  targets = [],
  findings = [],
  summary,
  onToggleTarget,
  onExportNow,
  onFixIssues,
  onSavePreset,
  onGenerateVariants,
}: DeliverySurfaceProps) {
  const [confirmExportOpen, setConfirmExportOpen] = useState(false);
  // Selected publishing targets share the render queue and upload preferences.
  const uploadAfterRender = useUploadPrefs((s) => s.enabled);
  const toggleUploadAfterRender = useUploadPrefs((s) => s.toggle);
  const queueEntries = useRenderQueueStore((s) => s.entries);
  const projectRoot = useProjectStore((s) => s.current);

  const resolvedTargets: DeliveryTarget[] = ALL_TARGETS.map((key) => {
    const provided = targets.find((t) => t.key === key);
    return provided ?? { key, active: false };
  });
  const activeTargetCount = resolvedTargets.filter((t) => t.active).length;
  const activeTargetKeys = resolvedTargets.filter((t) => t.active).map((t) => t.key);
  const counts = countBySeverity(findings);
  const blockingCount = counts.warning + counts.error + counts.failure;

  // Active, upload-capable targets get a "publish after render" toggle.
  const publishableActive = useMemo(
    () => resolvedTargets.filter((t) => t.active && isUploadCapableTarget(t.key)),
    [resolvedTargets],
  );
  // The metadata form renders for the intersection of "upload enabled" and
  // "selected target".
  const formTargets = useMemo<DeliveryTargetKey[]>(() => {
    const selected = new Set(resolvedTargets.filter((t) => t.active).map((t) => t.key));
    return Array.from(uploadAfterRender).filter((k) => selected.has(k));
  }, [resolvedTargets, uploadAfterRender]);
  const formDefaultTitle = useMemo(
    () => (formTargets.length === 0 ? "Untitled render" : TARGET_META[formTargets[0]].label),
    [formTargets],
  );

  // No dedicated AI-disclosure prop exists, so derive a soft signal
  // from the findings: if any finding talks about AI / synthetic /
  // generated media, surface a small disclosure chip.
  const aiDisclosed = useMemo(
    () =>
      findings.some((f) =>
        /\b(ai|synthetic|generated)\b/i.test(`${f.message} ${f.asset ?? ""}`),
      ),
    [findings],
  );

  function confirmExport() {
    setConfirmExportOpen(false);
    onExportNow?.();
  }

  return (
    <div className="h-full overflow-y-auto px-6 py-8">
      <div className="mx-auto flex w-full max-w-[640px] flex-col gap-7">
        {/* Header */}
        <div className="flex items-center gap-3">
          <div className="min-w-0">
            <h1 className="text-[var(--text-h1)] font-semibold leading-tight text-[var(--color-text-primary)]">
              Deliver
            </h1>
            <p className="mt-1 text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
              {activeTargetCount === 0
                ? "Pick where this goes, then export."
                : `${activeTargetCount} ${activeTargetCount === 1 ? "target" : "targets"} selected.`}
            </p>
          </div>
          {aiDisclosed ? (
            <span className="glass-content glow-violet ml-auto inline-flex shrink-0 items-center gap-1.5 rounded-full px-3 py-1.5 text-[11px] font-semibold text-[var(--color-text-primary)]">
              <Sparkles className="h-3.5 w-3.5 stroke-[1.75]" style={{ color: "#D8B4FE" }} />
              AI <span aria-hidden>⚠</span> disclosed
            </span>
          ) : null}
        </div>

        {/* Targets */}
        <section className="flex flex-col gap-3">
          <SheetSectionLabel>Targets</SheetSectionLabel>
          <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
            {resolvedTargets.map((target) => (
              <TargetChip
                key={target.key}
                target={target}
                onToggle={() => onToggleTarget?.(target.key)}
              />
            ))}
          </div>
        </section>

        {/* Publish after render — only for active, upload-capable targets.
            Opting in surfaces the metadata form below (title/description/
            visibility), for the selected publishing targets. */}
        {publishableActive.length > 0 ? (
          <section className="flex flex-col gap-3">
            <SheetSectionLabel>Publish after render</SheetSectionLabel>
            <div className="flex flex-col gap-1.5">
              {publishableActive.map((t) => (
                <PublishToggleRow
                  key={t.key}
                  target={t}
                  on={uploadAfterRender.has(t.key)}
                  onToggle={() => void toggleUploadAfterRender(t.key)}
                />
              ))}
            </div>
            {formTargets.length > 0 ? (
              <UploadMetadataForm
                selectedTargets={formTargets}
                jobIdHint={DRAFT_METADATA_JOB_ID}
                defaultTitle={formDefaultTitle}
              />
            ) : null}
          </section>
        ) : null}

        {/* Preflight */}
        <section className="flex flex-col gap-3">
          <SheetSectionLabel>
            Preflight
            {blockingCount > 0 ? (
              <span className="ml-2 font-normal text-[var(--color-text-muted)]">
                {blockingCount} to review
              </span>
            ) : null}
          </SheetSectionLabel>
          {findings.length === 0 ? (
            <PreflightRow
              severity="pass"
              message="No issues found — clean to export."
            />
          ) : (
            <div className="flex flex-col gap-1.5">
              {findings.map((f) => (
                <PreflightRow
                  key={f.id}
                  severity={f.severity}
                  message={f.message}
                  asset={f.asset}
                  time={f.time}
                />
              ))}
            </div>
          )}
        </section>

        {/* Render summary */}
        {summary ? (
          <section className="flex flex-col gap-3">
            <SheetSectionLabel>Render summary</SheetSectionLabel>
            <RenderSummaryCard summary={summary} outputs={activeTargetCount} />
          </section>
        ) : null}

        <section className="flex flex-col gap-3">
          <SheetSectionLabel>Campaign</SheetSectionLabel>
          <CampaignApprovalPanel
            sourceAssetId={projectRoot}
            selectedTargets={activeTargetKeys}
            renderEntries={queueEntries}
          />
        </section>

        {/* Render queue — appears once there's something rendering/queued so
            the sheet stays calm when idle.  */}
        {queueEntries.length > 0 ? (
          <section className="flex flex-col gap-3">
            <SheetSectionLabel>Render queue</SheetSectionLabel>
            <RenderQueuePanel />
          </section>
        ) : null}

        {/* Actions */}
        <section className="flex flex-col gap-3 pb-2">
          <button
            type="button"
            onClick={() => setConfirmExportOpen(true)}
            disabled={activeTargetCount === 0}
            className={cn(
              "glass-cta flex w-full items-center justify-center gap-2 rounded-2xl px-5 py-3.5 text-[15px]",
              activeTargetCount === 0 && "cursor-not-allowed opacity-50",
            )}
          >
            <Upload className="h-4 w-4 stroke-[2]" />
            Export now
          </button>
          {blockingCount > 0 ? (
            <button
              type="button"
              onClick={onFixIssues}
              className="glass-ghost glow-brand w-full rounded-xl px-4 py-2.5 text-[13px] font-medium"
            >
              Fix issues first
            </button>
          ) : null}
          <div className="grid grid-cols-2 gap-3">
            <button
              type="button"
              onClick={onSavePreset}
              className="glass-ghost inline-flex items-center justify-center gap-2 rounded-xl px-4 py-2.5 text-[13px]"
            >
              <Save className="h-3.5 w-3.5 stroke-[1.75]" />
              Save preset
            </button>
            <button
              type="button"
              onClick={onGenerateVariants}
              className="glass-ghost inline-flex items-center justify-center gap-2 rounded-xl px-4 py-2.5 text-[13px]"
            >
              <Download className="h-3.5 w-3.5 stroke-[1.75]" />
              Generate variants
            </button>
          </div>
        </section>
      </div>
      {confirmExportOpen ? (
        <ExportConfirmDialog
          targetCount={activeTargetCount}
          summary={summary}
          onCancel={() => setConfirmExportOpen(false)}
          onConfirm={confirmExport}
        />
      ) : null}
    </div>
  );
}

function SheetSectionLabel({ children }: { children: ReactNode }) {
  return (
    <h2 className="text-[var(--text-label)] font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-secondary)]">
      {children}
    </h2>
  );
}

/** A single platform/asset target as a glass toggle chip. */
function TargetChip({
  target,
  onToggle,
}: {
  target: DeliveryTarget;
  onToggle: () => void;
}) {
  const meta = TARGET_META[target.key];
  const label = target.label ?? meta.label;
  const spec = target.spec ?? meta.spec;
  return (
    <button
      type="button"
      role="switch"
      aria-checked={target.active}
      aria-label={`${label} — ${target.active ? "on" : "off"}`}
      onClick={onToggle}
      className={cn(
        "glass-content flex flex-col gap-2 rounded-2xl p-3.5 text-left transition-transform",
        "hover:-translate-y-px",
        target.active && "glow-brand",
      )}
    >
      <div className="flex items-start justify-between gap-2">
        <span
          className={cn(
            "grid h-8 w-8 shrink-0 place-items-center rounded-lg",
            target.active
              ? "bg-[rgba(239,68,68,0.16)]"
              : "bg-[rgba(255,255,255,0.05)]",
          )}
        >
          {meta.brand ? (
            <BrandIcon
              icon={meta.brand}
              tinted={target.active}
              className="h-4 w-4"
            />
          ) : (
            <meta.icon
              className={cn(
                "h-4 w-4 stroke-[1.75]",
                target.active
                  ? "text-[var(--color-brand)]"
                  : "text-[var(--color-text-secondary)]",
              )}
            />
          )}
        </span>
        {/* on/off pip */}
        <span
          className={cn(
            "mt-0.5 h-2.5 w-2.5 shrink-0 rounded-full transition-colors",
            target.active
              ? "bg-[var(--color-brand)] shadow-[0_0_8px_rgba(239,68,68,0.7)]"
              : "bg-[rgba(255,255,255,0.18)]",
          )}
        />
      </div>
      <div className="min-w-0">
        <div className="truncate text-[13px] font-semibold text-[var(--color-text-primary)]">
          {label}
        </div>
        <div className="truncate text-[11px] text-[var(--color-text-muted)]">
          {spec}
        </div>
      </div>
    </button>
  );
}

/**
 * Publish-after-render toggle for one active, upload-capable target.
 * A flat glass row (label + switch) — opting in feeds the metadata form
 * above the actions and routes the render to the provider's upload path.
 */
function PublishToggleRow({
  target,
  on,
  onToggle,
}: {
  target: DeliveryTarget;
  on: boolean;
  onToggle: () => void;
}) {
  const meta = TARGET_META[target.key];
  const label = target.label ?? meta.label;
  return (
    <div className="glass-content flex items-center gap-3 rounded-xl px-3.5 py-2.5">
      <span className="grid h-7 w-7 shrink-0 place-items-center rounded-lg bg-[rgba(239,68,68,0.16)]">
        {meta.brand ? (
          <BrandIcon icon={meta.brand} tinted className="h-4 w-4" />
        ) : (
          <meta.icon className="h-4 w-4 stroke-[1.75] text-[var(--color-brand)]" />
        )}
      </span>
      <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-[var(--color-text-primary)]">
        {label}
      </span>
      <button
        type="button"
        role="switch"
        aria-checked={on}
        aria-label={`Upload to ${label} after render — ${on ? "on" : "off"}`}
        onClick={onToggle}
        className={cn(
          "inline-flex items-center gap-2 rounded-full px-3 py-1.5 text-[11px] font-semibold transition-colors",
          on ? "glass-cta" : "glass-ghost text-[var(--color-text-secondary)]",
        )}
      >
        <Upload className="h-3.5 w-3.5 stroke-[1.75]" />
        Upload {on ? "on" : "off"}
      </button>
    </div>
  );
}

/** Quiet preflight row: severity dot + icon + message. */
function PreflightRow({
  severity,
  message,
  asset,
  time,
}: {
  severity: PreflightSeverity;
  message: string;
  asset?: string;
  time?: string;
}) {
  const { color, Icon } = severityVisual(severity);
  return (
    <div className="flex items-start gap-2.5 px-1 py-1.5">
      <Icon
        className="mt-0.5 h-4 w-4 shrink-0 stroke-[1.75]"
        style={{ color }}
        aria-hidden
      />
      <div className="min-w-0 flex-1">
        <p className="text-[13px] leading-snug text-[var(--color-text-primary)]">
          {message}
        </p>
        {asset || time ? (
          <p className="mt-0.5 text-[11px] text-[var(--color-text-muted)]">
            {[time, asset].filter(Boolean).join(" · ")}
          </p>
        ) : null}
      </div>
    </div>
  );
}

function severityVisual(severity: PreflightSeverity): {
  color: string;
  Icon: typeof CheckCircle2;
} {
  switch (severity) {
    case "error":
    case "failure":
      return { color: "var(--color-danger)", Icon: XCircle };
    case "warning":
      return { color: "var(--color-warning)", Icon: AlertTriangle };
    default:
      return { color: "var(--color-success)", Icon: CheckCircle2 };
  }
}

/** Summary of the configured render outputs. */
function RenderSummaryCard({
  summary,
  outputs,
}: {
  summary: DeliveryRenderSummary;
  outputs: number;
}) {
  const outputCount = summary.outputs || outputs;
  return (
    <div className="glass-content rounded-2xl p-4">
      <div className="grid grid-cols-2 gap-x-6 gap-y-3">
        <SummaryStat label="Duration" value={summary.duration} />
        <SummaryStat
          label="Outputs"
          value={`${outputCount} ${outputCount === 1 ? "file" : "files"}`}
        />
        {summary.estimatedSize ? (
          <SummaryStat label="Est. size" value={summary.estimatedSize} />
        ) : null}
      </div>

    </div>
  );
}

function SummaryStat({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0">
      <div className="text-[11px] uppercase tracking-wide text-[var(--color-text-muted)]">
        {label}
      </div>
      <div className="truncate text-[14px] font-semibold text-[var(--color-text-primary)]">
        {value}
      </div>
    </div>
  );
}

function ExportConfirmDialog({
  targetCount,
  summary,
  onCancel,
  onConfirm,
}: {
  targetCount: number;
  summary?: DeliveryRenderSummary;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <header className="modal-header">
          <h2>Start export?</h2>
          <button className="modal-close" onClick={onCancel} aria-label="Close">
            ×
          </button>
        </header>
        <div className="modal-body">
          <Stack gap="3">
            <p>
              This will render and write delivery files for {targetCount} selected{" "}
              {targetCount === 1 ? "target" : "targets"}.
            </p>
            {summary ? (
              <div className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] p-3">
                <KV label="Duration" value={summary.duration} />
                {summary.estimatedSize ? (
                  <KV label="Estimated size" value={summary.estimatedSize} />
                ) : null}
              </div>
            ) : null}
            <p className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
              After export finishes, Montage will keep the output in review until you mark it good.
            </p>
          </Stack>
        </div>
        <footer className="modal-footer">
          <button type="button" onClick={onCancel}>
            Cancel
          </button>
          <button type="button" className="primary" onClick={onConfirm}>
            Start render
          </button>
        </footer>
      </div>
    </div>
  );
}
