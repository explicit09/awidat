import {
  Download,
  Play,
  Save,
  Upload,
} from "lucide-react";
import { useMemo, useState, type ReactNode } from "react";
import {
  Button,
  Inline,
  Stack,
  cn,
  type PreflightSeverity,
} from "../ui";
import { useMode } from "../state/mode";
import { useUploadPrefs } from "../state/uploadPrefs";
import {
  useRenderQueueStore,
  type RenderQueueEntry,
} from "../app/renderQueue";
import { TargetsList } from "./delivery/TargetsList";
import { PreflightPanel } from "./delivery/PreflightPanel";
import { SafeAreaPreview } from "./delivery/SafeAreaPreview";
import { IssueInspector } from "./delivery/IssueInspector";
import { RenderSummary, KV } from "./delivery/RenderSummary";
import { RenderQueuePanel } from "./delivery/RenderQueue";
import { UploadMetadataForm } from "./delivery/UploadMetadataForm";
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
export const DRAFT_METADATA_JOB_ID = "awidat.deliver.draft";

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
  onAgentRepair,
}: DeliverySurfaceProps) {
  const mode = useMode((s) => s.mode);
  const [severityFilter, setSeverityFilter] = useState<"all" | PreflightSeverity>("all");
  const [confirmExportOpen, setConfirmExportOpen] = useState(false);
  const queueEntries = useRenderQueueStore((s) => s.entries);
  const uploadAfterRender = useUploadPrefs((s) => s.enabled);
  const toggleUploadAfterRender = useUploadPrefs((s) => s.toggle);

  // Resolve targets so all 6 are always rendered.
  const resolvedTargets: DeliveryTarget[] = ALL_TARGETS.map((key) => {
    const provided = targets.find((t) => t.key === key);
    return provided ?? { key, active: false };
  });
  const activeTargetCount = resolvedTargets.filter((target) => target.active).length;
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
              <RenderQueuePanel />
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
              After export finishes, Awidat will keep the output in review until you mark it good.
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
