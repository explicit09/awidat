import {
  AlertTriangle,
  CheckCircle2,
  Download,
  Play,
  Save,
  Sparkles,
  Upload,
  XCircle,
} from "lucide-react";
import { useMemo, useState, type ReactNode } from "react";
import {
  BrandIcon,
  Button,
  Inline,
  Stack,
  cn,
  type PreflightSeverity,
} from "../ui";
import { useMode } from "../state/mode";
import { useProjectStore } from "../app/state";
import { useUploadPrefs } from "../state/uploadPrefs";
import {
  useRenderQueueStore,
  type RenderQueueEntry,
} from "../app/renderQueue";
import { TargetsList, isUploadCapableTarget } from "./delivery/TargetsList";
import { PreflightPanel } from "./delivery/PreflightPanel";
import { SafeAreaPreview } from "./delivery/SafeAreaPreview";
import { IssueInspector } from "./delivery/IssueInspector";
import { RenderSummary, KV } from "./delivery/RenderSummary";
import { RenderQueuePanel } from "./delivery/RenderQueue";
import { UploadMetadataForm } from "./delivery/UploadMetadataForm";
import { CampaignApprovalPanel } from "./delivery/CampaignApprovalPanel";
import { SocialPublish } from "../app/social/SocialPublish";
import { TARGET_META, targetKeyForKind } from "./delivery/targetMeta";
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

/**
 * DeliverySurface — concept Screen 7.
 *
 * Three columns: platform target preset rail on the left, preflight
 * checklist in the center, render summary + delivery confidence on the
 * right. The user picks targets, addresses findings, and exports —
 * with explicit signal about what's risky.
 *
 * Mode-aware: in Creator mode the right-column detail blocks (render
 * summary, render queue) collapse behind a "Show details" disclosure
 * so the export button stays the focus. Pro mode keeps everything
 * visible. Mirrors the IndexRailCreator pattern.
 */

export type DeliverySurfaceProps = {
  targets?: DeliveryTarget[];
  findings?: PreflightFinding[];
  summary?: DeliveryRenderSummary;
  onToggleTarget?: (key: DeliveryTargetKey) => void;
  onExportNow?: () => void;
  onFixIssues?: () => void;
  onSavePreset?: () => void;
  onGenerateVariants?: () => void;
  onAgentRepair?: (finding: PreflightFinding) => void;
  /**
   * Layout flavor.
   *   "cockpit" (default) — the legacy dense 3-column AppShell cockpit.
   *   "sheet"             — the calm, single-column glass layout that
   *                         lives inside the 2026 Stage glass sheet.
   * The cockpit path is preserved byte-for-byte; the sheet path is a
   * pure restyle/re-arrange over the SAME props + handlers.
   */
  variant?: "cockpit" | "sheet";
};

export function DeliverySurface(props: DeliverySurfaceProps) {
  if (props.variant === "sheet") return <DeliverySheet {...props} />;
  return <DeliveryCockpit {...props} />;
}

function DeliveryCockpit({
  targets = [],
  findings = [],
  summary,
  onToggleTarget,
  onExportNow,
  onFixIssues,
  onSavePreset,
  onGenerateVariants,
  onAgentRepair,
}: DeliverySurfaceProps) {
  const mode = useMode((s) => s.mode);
  const [severityFilter, setSeverityFilter] = useState<"all" | PreflightSeverity>("all");
  const [confirmExportOpen, setConfirmExportOpen] = useState(false);
  const queueEntries = useRenderQueueStore((s) => s.entries);
  const projectRoot = useProjectStore((s) => s.current);
  const uploadAfterRender = useUploadPrefs((s) => s.enabled);
  const toggleUploadAfterRender = useUploadPrefs((s) => s.toggle);

  // Resolve targets so all 6 are always rendered.
  const resolvedTargets: DeliveryTarget[] = ALL_TARGETS.map((key) => {
    const provided = targets.find((t) => t.key === key);
    return provided ?? { key, active: false };
  });
  const activeTargetCount = resolvedTargets.filter((target) => target.active).length;
  const activeTargetKeys = resolvedTargets
    .filter((target) => target.active)
    .map((target) => target.key);
  const runningRender = [...queueEntries]
    .reverse()
    .find((entry) => entry.status === "running" || entry.status === "pending");
  const pendingReview = [...queueEntries]
    .reverse()
    .find((entry) => entry.status === "done" && entry.reviewStatus === "pending");

  // Map of target key -> percent for any running entry. Lets the left
  // rail show a `<StatusPill family="job" state="running" percent={…}/>`
  // next to the target that's actively rendering.
  const runningByTarget = useMemo(() => {
    const out: Partial<Record<DeliveryTargetKey, number>> = {};
    for (const entry of queueEntries) {
      if (entry.status !== "running") continue;
      const key = targetKeyForKind(entry.kind, entry.label);
      if (!key) continue;
      out[key] = typeof entry.progress === "number" ? entry.progress : 0;
    }
    return out;
  }, [queueEntries]);

  // Publishing targets the user has opted into upload-after-render
  // *and* has currently selected as a delivery target. The form only
  // renders for the intersection — picking "YouTube" without
  // toggling Upload doesn't surface the form.
  const formTargets = useMemo<DeliveryTargetKey[]>(() => {
    const selected = new Set(
      resolvedTargets.filter((t) => t.active).map((t) => t.key),
    );
    return Array.from(uploadAfterRender).filter((k) => selected.has(k));
  }, [resolvedTargets, uploadAfterRender]);
  // Default title for newly-touched targets. We pick the most
  // information-bearing render-target label as the seed.
  const formDefaultTitle = useMemo(() => {
    if (formTargets.length === 0) return "Untitled render";
    return TARGET_META[formTargets[0]].label;
  }, [formTargets]);

  const counts = countBySeverity(findings);
  const filtered = severityFilter === "all"
    ? findings
    : findings.filter((f) => f.severity === severityFilter);

  const selectedIssue = findings.find(
    (f) => f.severity === "warning" || f.severity === "error" || f.severity === "failure",
  ) ?? findings[0];

  function confirmExport() {
    setConfirmExportOpen(false);
    onExportNow?.();
  }

  return (
    <div className="grid h-full grid-cols-[292px_minmax(0,1fr)_334px] bg-[var(--color-surface-app)] min-h-0">
      {/* LEFT — Targets list */}
      <aside className="border-r border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] flex flex-col min-h-0">
        <TargetsList
          resolvedTargets={resolvedTargets}
          findings={findings}
          runningByTarget={runningByTarget}
          uploadAfterRender={uploadAfterRender}
          onToggleTarget={onToggleTarget}
          onToggleUploadAfterRender={(key) => {
            void toggleUploadAfterRender(key);
          }}
          onAgentRepair={onAgentRepair}
        />
      </aside>

      {/* CENTER — Preflight + Safe-area preview */}
      <main className="grid min-h-0 grid-rows-[minmax(0,1fr)_minmax(220px,260px)]">
        <PreflightPanel
          findings={findings}
          filtered={filtered}
          counts={counts}
          severityFilter={severityFilter}
          setSeverityFilter={setSeverityFilter}
          selectedIssue={selectedIssue}
          onAgentRepair={onAgentRepair}
        />
        <SafeAreaPreview />
      </main>

      {/* RIGHT — Issue inspector + Render summary + Render queue + actions */}
      <aside className="border-l border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] flex flex-col min-h-0">
        <div className="h-8 px-3 flex items-center border-b border-[var(--color-border-subtle)] shrink-0">
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
            Issue inspector
          </span>
        </div>
        <div className="flex-1 overflow-y-auto p-2.5">
          <Stack gap="3">
            <RenderStatusBanner
              running={runningRender}
              pendingReview={pendingReview}
            />
            <IssueInspector
              selectedIssue={selectedIssue}
              onAgentRepair={onAgentRepair}
            />
            {formTargets.length > 0 ? (
              <UploadMetadataForm
                selectedTargets={formTargets}
                jobIdHint={DRAFT_METADATA_JOB_ID}
                defaultTitle={formDefaultTitle}
              />
            ) : null}
            <RightColumnDetails mode={mode}>
              {summary ? <RenderSummary summary={summary} /> : null}
              <CampaignApprovalPanel
                sourceAssetId={projectRoot}
                selectedTargets={activeTargetKeys}
                renderEntries={queueEntries}
              />
              <RenderQueuePanel />
              {/* Server-backed publishing: connect an account + schedule a
                  finished render to it (social_bind/validate/schedule/upload). */}
              <SocialPublish />
            </RightColumnDetails>
            <Stack gap="2">
              <Button
                variant="primary"
                size="md"
                onClick={() => setConfirmExportOpen(true)}
                trailingIcon={
                  <span className="inline-flex items-center gap-1.5">
                    <span className="rounded-[var(--radius-xs)] border border-[rgba(0,0,0,0.18)] bg-[rgba(0,0,0,0.18)] px-1 font-mono text-[10px] leading-[14px] text-[var(--color-text-inverse)]/85">
                      ⌘E
                    </span>
                    <Upload className="h-3.5 w-3.5 stroke-[1.75]" />
                  </span>
                }
              >
                Export now
              </Button>
              {counts.warning + counts.error + counts.failure > 0 ? (
                <Button variant="secondary" size="md" onClick={onFixIssues}>
                  Fix issues first
                </Button>
              ) : null}
              <Button
                variant="ghost"
                size="sm"
                onClick={onSavePreset}
                leadingIcon={<Save className="h-3.5 w-3.5 stroke-[1.75]" />}
              >
                Save preset
              </Button>
              <Button
                variant="ghost"
                size="sm"
                onClick={onGenerateVariants}
                leadingIcon={<Download className="h-3.5 w-3.5 stroke-[1.75]" />}
              >
                Generate platform variants
              </Button>
            </Stack>
          </Stack>
        </div>
      </aside>
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

/* =====================================================================
   SHEET LAYOUT — calm, single-column, glass-sheet-native (2026 Stage)
   Same props + handlers as the cockpit; only the arrangement + skin
   change. A centered readable column of glass cards instead of the
   dense 3-column cockpit.
   ===================================================================== */

function DeliverySheet({
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
  // Publish wiring — shared with the cockpit so the Stage sheet can opt
  // targets into upload-after-render, fill metadata, and watch the queue.
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
  // "selected target" — mirrors the cockpit's formTargets contract exactly.
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
            visibility), matching the cockpit's right-column publish flow. */}
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
            the sheet stays calm when idle. Same panel the cockpit uses. */}
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
              ? "bg-[rgba(255,122,24,0.16)]"
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
              ? "bg-[var(--color-brand)] shadow-[0_0_8px_rgba(255,122,24,0.7)]"
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
      <span className="grid h-7 w-7 shrink-0 place-items-center rounded-lg bg-[rgba(255,122,24,0.16)]">
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

/** Compact render-summary glass card with a delivery-confidence bar. */
function RenderSummaryCard({
  summary,
  outputs,
}: {
  summary: DeliveryRenderSummary;
  outputs: number;
}) {
  const confidence = Math.max(0, Math.min(1, summary.confidence));
  const pct = Math.round(confidence * 100);
  const outputCount = summary.outputs || outputs;
  // Confidence reads warm(green)→amber→red as it drops.
  const confColor =
    confidence >= 0.85
      ? "var(--color-success)"
      : confidence >= 0.6
        ? "var(--color-warning)"
        : "var(--color-danger)";
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
      <div className="mt-4 flex flex-col gap-1.5">
        <div className="flex items-center justify-between">
          <span className="text-[11px] font-medium text-[var(--color-text-secondary)]">
            Delivery confidence
          </span>
          <span
            className="font-mono text-[11px] font-semibold"
            style={{ color: confColor }}
          >
            {pct}%
          </span>
        </div>
        <div className="h-2 overflow-hidden rounded-full bg-[rgba(255,255,255,0.08)]">
          <div
            className="h-full rounded-full transition-[width] duration-300"
            style={{ width: `${pct}%`, background: confColor }}
          />
        </div>
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

/** Right-column detail wrapper. In Pro mode it just renders the
 *  children. In Creator mode it collapses them behind a single
 *  "Show details" toggle — mirrors the IndexRailCreator pattern so
 *  the Deliver right column doesn't overwhelm new users. */
function RightColumnDetails({
  mode,
  children,
}: {
  mode: "pro" | "creator";
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  if (mode === "pro") return <>{children}</>;
  return (
    <Stack gap="2">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className={cn(
          "self-start rounded text-left text-[11px] font-semibold transition-colors",
          "text-[var(--color-brand)] hover:text-[var(--color-text-primary)]",
        )}
      >
        {open ? "▴ Hide details" : "▾ Show details"}{" "}
        <span className="text-[var(--color-text-muted)]">
          · render summary + queue
        </span>
      </button>
      {open ? <>{children}</> : null}
    </Stack>
  );
}

function RenderStatusBanner({
  running,
  pendingReview,
}: {
  running?: RenderQueueEntry;
  pendingReview?: RenderQueueEntry;
}) {
  if (running) {
    const progress =
      typeof running.progress === "number"
        ? Math.max(0, Math.min(100, running.progress))
        : null;
    return (
      <div className="rounded-[var(--radius-md)] border border-[rgba(56,189,248,0.45)] bg-[rgba(56,189,248,0.12)] p-3 shadow-[0_12px_32px_rgba(0,0,0,0.24)]">
        <Stack gap="3">
          <Inline justify="between" align="center">
            <Inline gap="2" align="center" className="min-w-0">
              <span className="h-2.5 w-2.5 shrink-0 rounded-full bg-[var(--color-brand-secondary)] animate-pulse" />
              <span className="text-[var(--text-h3)] font-semibold text-[var(--color-text-primary)]">
                Rendering final output
              </span>
            </Inline>
            <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-secondary)]">
              {progress === null ? "working" : `${Math.round(progress)}%`}
            </span>
          </Inline>
          <p className="text-[var(--text-body-sm)] leading-snug text-[var(--color-text-secondary)]">
            {running.label} is being exported. Keep this project open until the render finishes.
          </p>
          <div className="h-2 overflow-hidden rounded-full bg-[var(--color-surface-input)]">
            <div
              className="h-full rounded-full bg-[var(--color-brand-secondary)] transition-[width] duration-300"
              style={{ width: `${progress ?? 12}%` }}
            />
          </div>
        </Stack>
      </div>
    );
  }
  if (pendingReview) {
    return (
      <div className="rounded-[var(--radius-md)] border border-[rgba(245,158,11,0.5)] bg-[rgba(245,158,11,0.1)] p-3">
        <Stack gap="2">
          <Inline gap="2" align="center">
            <Play className="h-4 w-4 stroke-[1.75] text-[var(--color-warning)]" />
            <span className="text-[var(--text-h3)] font-semibold text-[var(--color-text-primary)]">
              Review final render
            </span>
          </Inline>
          <p className="text-[var(--text-body-sm)] leading-snug text-[var(--color-text-secondary)]">
            {pendingReview.label} finished. Watch the output before treating it as ready to deliver.
          </p>
        </Stack>
      </div>
    );
  }
  return null;
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
