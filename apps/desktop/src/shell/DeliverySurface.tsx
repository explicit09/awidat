import {
  Captions,
  CircleCheck,
  Download,
  FileImage,
  FileVideo,
  Image as ImageIcon,
  PackageCheck,
  Play,
  Save,
  Settings as SettingsIcon,
  ShieldCheck,
  Square,
  Upload,
  type LucideIcon,
} from "lucide-react";
import { useState, type ReactNode } from "react";
import {
  Button,
  Card,
  ConfidenceMeter,
  Inline,
  PreflightFindingRow,
  Stack,
  cn,
  type PreflightSeverity,
} from "../ui";

/**
 * DeliverySurface — concept Screen 7.
 *
 * Three columns: platform target preset rail on the left, preflight
 * checklist in the center, render summary + delivery confidence on the
 * right. The user picks targets, addresses findings, and exports —
 * with explicit signal about what's risky.
 */

export type DeliveryTargetKey =
  | "youtube"
  | "tiktok"
  | "instagram"
  | "captions"
  | "cover"
  | "custom";

export type DeliveryTarget = {
  key: DeliveryTargetKey;
  /** Selected by the user. */
  active: boolean;
  /** Optional human label override. */
  label?: string;
  /** Optional one-line spec ("1080p · 16:9 · h264"). */
  spec?: string;
};

export type PreflightFinding = {
  id: string;
  severity: PreflightSeverity;
  time?: string;
  message: string;
  asset?: string;
  suggestedFix?: string;
};

export type DeliveryRenderSummary = {
  duration: string;
  estimatedSize?: string;
  outputs: number;
  /** 0..1 — how confident the system is that the render will be clean. */
  confidence: number;
};

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

const TARGET_META: Record<
  DeliveryTargetKey,
  { icon: LucideIcon; label: string; spec: string; kind: "video" | "asset" }
> = {
  youtube: { icon: Play, label: "YouTube", spec: "1080p · 16:9 · h264", kind: "video" },
  tiktok: { icon: FileVideo, label: "TikTok", spec: "1080p · 9:16 · h264", kind: "video" },
  instagram: { icon: Square, label: "Instagram", spec: "1080p · 1:1 / 9:16", kind: "video" },
  captions: { icon: Captions, label: "Captions", spec: "SRT + VTT", kind: "asset" },
  cover: { icon: ImageIcon, label: "Cover", spec: "1280×720 PNG", kind: "asset" },
  custom: { icon: FileImage, label: "Custom frame", spec: "User-selected", kind: "asset" },
};

const ALL_TARGETS: DeliveryTargetKey[] = [
  "youtube",
  "tiktok",
  "instagram",
  "captions",
  "cover",
  "custom",
];

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
  const [severityFilter, setSeverityFilter] = useState<"all" | PreflightSeverity>("all");

  // Resolve targets so all 6 are always rendered.
  const resolvedTargets: DeliveryTarget[] = ALL_TARGETS.map((key) => {
    const provided = targets.find((t) => t.key === key);
    return provided ?? { key, active: false };
  });

  const counts = countBySeverity(findings);
  const filtered = severityFilter === "all"
    ? findings
    : findings.filter((f) => f.severity === severityFilter);

  return (
    <div className="grid h-full grid-cols-[260px_1fr_320px] bg-[var(--color-surface-app)] min-h-0">
      {/* Targets */}
      <aside className="border-r border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] flex flex-col min-h-0">
        <div className="h-10 px-3 flex items-center border-b border-[var(--color-border-subtle)] shrink-0">
          <Inline gap="2" align="center">
            <PackageCheck className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-text-muted)]" />
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
              Targets
            </span>
          </Inline>
        </div>
        <div className="flex-1 overflow-y-auto p-2">
          <Stack gap="1">
            {resolvedTargets.map((t) => {
              const meta = TARGET_META[t.key];
              const Icon = meta.icon;
              return (
                <button
                  key={t.key}
                  type="button"
                  onClick={() => onToggleTarget?.(t.key)}
                  aria-pressed={t.active}
                  className={cn(
                    "w-full text-left rounded-[var(--radius-md)] border px-2.5 py-2 transition-colors",
                    t.active
                      ? "border-[var(--color-border-active)] bg-[var(--color-surface-selected)] glow-active"
                      : "border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] hover:border-[var(--color-border)] hover:bg-[var(--color-surface-card-hover)]",
                  )}
                >
                  <Inline gap="2" align="center">
                    <Icon className="h-4 w-4 stroke-[1.75] text-[var(--color-text-primary)] shrink-0" />
                    <Stack gap="0" className="min-w-0 flex-1">
                      <span className="text-[var(--text-body-sm)] font-medium text-[var(--color-text-primary)] truncate">
                        {t.label ?? meta.label}
                      </span>
                      <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] truncate">
                        {t.spec ?? meta.spec}
                      </span>
                    </Stack>
                    {t.active ? (
                      <CircleCheck className="h-4 w-4 stroke-[1.75] text-[var(--color-success)] shrink-0" />
                    ) : null}
                  </Inline>
                </button>
              );
            })}
          </Stack>
        </div>
      </aside>

      {/* Preflight */}
      <main className="flex flex-col min-h-0">
        <div className="h-10 px-4 flex items-center justify-between border-b border-[var(--color-border-subtle)] shrink-0">
          <Inline gap="2" align="center">
            <ShieldCheck className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-text-muted)]" />
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
              Preflight
            </span>
            <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
              {findings.length} {findings.length === 1 ? "finding" : "findings"}
            </span>
          </Inline>
          <Inline gap="1" align="center">
            <SeverityChip label="All" count={findings.length} active={severityFilter === "all"} onClick={() => setSeverityFilter("all")} />
            <SeverityChip label="Pass" count={counts.pass} color="var(--color-success)" active={severityFilter === "pass"} onClick={() => setSeverityFilter("pass")} />
            <SeverityChip label="Warn" count={counts.warning} color="var(--color-warning)" active={severityFilter === "warning"} onClick={() => setSeverityFilter("warning")} />
            <SeverityChip label="Error" count={counts.error + counts.failure} color="var(--color-danger)" active={severityFilter === "error"} onClick={() => setSeverityFilter("error")} />
          </Inline>
        </div>
        <div className="flex-1 overflow-y-auto p-3">
          {filtered.length === 0 ? (
            <EmptyPreflight />
          ) : (
            <Stack gap="2">
              {filtered.map((f) => (
                <PreflightFindingRow
                  key={f.id}
                  severity={f.severity}
                  time={f.time}
                  message={f.message}
                  asset={f.asset}
                  suggestedFix={f.suggestedFix}
                  actions={
                    onAgentRepair && (f.severity === "warning" || f.severity === "error" || f.severity === "failure") ? (
                      <Button
                        variant="repair"
                        size="xs"
                        onClick={(e) => {
                          e.stopPropagation();
                          onAgentRepair(f);
                        }}
                      >
                        Repair
                      </Button>
                    ) : undefined
                  }
                />
              ))}
            </Stack>
          )}
        </div>
      </main>

      {/* Summary */}
      <aside className="border-l border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] flex flex-col min-h-0">
        <div className="h-10 px-3 flex items-center border-b border-[var(--color-border-subtle)] shrink-0">
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
            Render summary
          </span>
        </div>
        <div className="flex-1 overflow-y-auto p-3">
          <Stack gap="4">
            {summary ? (
              <Card padding="md">
                <Stack gap="3">
                  <KV label="Duration" value={summary.duration} />
                  <KV
                    label="Outputs"
                    value={`${summary.outputs} ${summary.outputs === 1 ? "target" : "targets"}`}
                  />
                  {summary.estimatedSize ? (
                    <KV label="Est. size" value={summary.estimatedSize} />
                  ) : null}
                  <Stack gap="1">
                    <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                      Delivery confidence
                    </span>
                    <ConfidenceMeter score={summary.confidence} label="" size="sm" />
                  </Stack>
                </Stack>
              </Card>
            ) : null}
            <Stack gap="2">
              <Button
                variant="primary"
                size="md"
                onClick={onExportNow}
                trailingIcon={<Upload className="h-3.5 w-3.5 stroke-[1.75]" />}
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
    </div>
  );
}

function SeverityChip({
  label,
  count,
  color,
  active,
  onClick,
}: {
  label: string;
  count: number;
  color?: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        "inline-flex items-center gap-1.5 h-6 px-2 rounded-[var(--radius-xs)]",
        "text-[var(--text-caption)] font-medium border transition-colors",
        active
          ? "bg-[var(--color-surface-selected)] border-[var(--color-border-active)] text-[var(--color-text-primary)]"
          : "bg-[var(--color-surface-card)] border-[var(--color-border-subtle)] text-[var(--color-text-secondary)] hover:border-[var(--color-border)] hover:text-[var(--color-text-primary)]",
      )}
    >
      {color ? <span className="h-1.5 w-1.5 rounded-full" style={{ backgroundColor: color }} /> : null}
      <span>{label}</span>
      <span className="font-mono text-[var(--color-text-muted)]">{count}</span>
    </button>
  );
}

function EmptyPreflight() {
  return (
    <div className="flex h-full items-center justify-center p-8">
      <Stack gap="2" align="center" className="text-center">
        <ShieldCheck className="h-8 w-8 stroke-[1.5] text-[var(--color-success)]" />
        <span className="text-[var(--text-h3)] font-semibold text-[var(--color-text-primary)]">
          Nothing to flag
        </span>
        <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)] max-w-sm">
          Preflight didn't find any issues for the selected targets.
          You can export when ready.
        </span>
      </Stack>
    </div>
  );
}

function countBySeverity(findings: PreflightFinding[]) {
  const c: Record<PreflightSeverity, number> = {
    pass: 0,
    info: 0,
    warning: 0,
    error: 0,
    failure: 0,
  };
  for (const f of findings) c[f.severity]++;
  return c;
}

function KV({ label, value }: { label: string; value: string | ReactNode }) {
  return (
    <Inline justify="between" align="baseline">
      <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        {label}
      </span>
      <span className="font-mono text-[var(--text-body-sm)] text-[var(--color-text-primary)]">{value}</span>
    </Inline>
  );
}

// Suppress unused warning — SettingsIcon kept available for a settings-shortcut iteration.
void SettingsIcon;
