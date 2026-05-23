import {
  Captions,
  CircleCheck,
  Download,
  FileImage,
  FileVideo,
  Image as ImageIcon,
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
import podcastWide from "./assets/podcast-wide.jpg";
import {
  useRenderQueueStore,
  type RenderQueueEntry,
} from "../app/renderQueue";

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
  const activeTargetCount = resolvedTargets.filter((target) => target.active).length;

  const counts = countBySeverity(findings);
  const filtered = severityFilter === "all"
    ? findings
    : findings.filter((f) => f.severity === severityFilter);

  const selectedIssue = findings.find((f) => f.severity === "warning" || f.severity === "error" || f.severity === "failure") ?? findings[0];

  return (
    <div className="grid h-full grid-cols-[292px_minmax(0,1fr)_334px] bg-[var(--color-surface-app)] min-h-0">
      {/* Targets + repair affordance — replaces the duplicated agent
          rail. The Edit-tab CommandRail owns the agent on its tab;
          Deliver is about picking outputs and shipping them. */}
      <aside className="border-r border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] flex flex-col min-h-0">
        <TargetsRail
          resolvedTargets={resolvedTargets}
          findings={findings}
          onToggleTarget={onToggleTarget}
          onAgentRepair={onAgentRepair}
        />
        <div hidden>
        <div className="border-b border-[var(--color-border-subtle)] p-2.5">
          <Stack gap="2">
            <Inline justify="between" align="center">
              <Inline gap="2" align="center">
                <span className="text-[var(--color-brand-secondary)]">›</span>
                <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
                  Agent command
                </span>
              </Inline>
              <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">Delivery</span>
            </Inline>
            <div className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2.5">
              <p className="text-[var(--text-body-sm)] leading-relaxed text-[var(--color-text-primary)]">
                Check this project for delivery and fix anything that could affect export quality.
              </p>
            </div>
            <div className="grid grid-cols-2 gap-2">
              <DeliveryContextChip label="Clip: Interview_A" tone="blue" />
              <DeliveryContextChip label="Range: 00:12-18:40" tone="blue" />
              <DeliveryContextChip label="Transcript region selected" tone="green" />
              <DeliveryContextChip label="Target: Delivery" tone="purple" />
            </div>
          </Stack>
        </div>
        <div className="border-b border-[var(--color-border-subtle)] p-2.5">
          <Stack gap="2">
            <Inline justify="between" align="baseline">
              <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
                Agent plan
              </span>
              <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">100%</span>
            </Inline>
            <div className="h-1.5 overflow-hidden rounded-full bg-[var(--color-surface-input)]">
              <div className="h-full rounded-full bg-[var(--color-success)]" style={{ width: "100%" }} />
            </div>
            <span className="text-[var(--text-body-sm)] font-medium text-[var(--color-text-primary)]">
              Delivery / Preflight
            </span>
            {[
              "Analyze timeline & media",
              "Run technical preflight",
              "Check captions & safe areas",
              "Validate delivery requirements",
              "Estimate render & file size",
            ].map((item, index) => (
              <div key={item} className="grid grid-cols-[20px_1fr_auto] items-center gap-2">
                <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">{index + 1}.</span>
                <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)]">{item}</span>
                <CircleCheck className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-success)]" />
              </div>
            ))}
            <Inline justify="between" align="baseline">
              <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">Est. time remaining</span>
              <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">00:00:00</span>
            </Inline>
          </Stack>
        </div>
        <div className="flex-1 overflow-y-auto p-2.5">
          <Stack gap="2">
            <Inline justify="between" align="baseline">
              <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
                Activity log
              </span>
              <span className="rounded-full bg-[var(--color-surface-card)] px-2 py-0.5 font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
                {findings.length}
              </span>
            </Inline>
            {[
              "Checked active delivery preset",
              `${activeTargetCount} target${activeTargetCount === 1 ? "" : "s"} selected for export`,
              `${counts.warning} warnings and ${counts.error + counts.failure} failures flagged`,
            ].map((item, index) => (
              <div key={item} className="grid grid-cols-[24px_1fr] gap-2 text-[var(--text-caption)] text-[var(--color-text-secondary)]">
                <span className="font-mono text-[var(--color-text-muted)]">{String(index + 1).padStart(2, "0")}</span>
                <span className="leading-snug">{item}</span>
              </div>
            ))}
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
              Suggested next actions
            </span>
            <DeliverySuggestion
              icon={<Download />}
              title="Generate platform variants"
              detail="Auto reframe for TikTok, Shorts, Reels"
              onClick={onGenerateVariants}
            />
            <DeliverySuggestion
              icon={<Captions />}
              title="Burn in captions"
              detail="Create a captioned version"
              onClick={() => onToggleTarget?.("captions")}
            />
            <DeliverySuggestion
              icon={<Save />}
              title="Save delivery preset"
              detail="Save these settings as a preset"
              onClick={onSavePreset}
            />
          </Stack>
        </div>
        </div>
      </aside>

      {/* Preflight */}
      <main className="grid min-h-0 grid-rows-[98px_minmax(0,1fr)_190px]">
        <section className="border-b border-[var(--color-border-subtle)] p-1.5">
          <div className="grid h-full grid-cols-6 gap-2">
            {resolvedTargets.map((target) => {
              const meta = TARGET_META[target.key];
              const Icon = meta.icon;
              return (
                <button
                  key={`preset-${target.key}`}
                  type="button"
                  onClick={() => onToggleTarget?.(target.key)}
                  aria-pressed={target.active}
                  className={cn(
                    "rounded-[var(--radius-md)] border px-2 py-1.5 text-left transition-colors",
                    target.active
                      ? "border-[var(--color-border-active)] bg-[var(--color-surface-selected)] glow-active"
                      : "border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] hover:border-[var(--color-border)]",
                  )}
                >
                  <Stack gap="1">
                    <Inline justify="between" align="center">
                      <Icon className="h-4 w-4 stroke-[1.5] text-[var(--color-text-primary)]" />
                      {target.active ? <CircleCheck className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-success)]" /> : null}
                    </Inline>
                    <Stack gap="0">
                      <span className="truncate text-[var(--text-body-sm)] font-semibold text-[var(--color-text-primary)]">
                        {target.label ?? meta.label}
                      </span>
                      <span className="truncate text-[var(--text-caption)] text-[var(--color-text-muted)]">
                        {target.spec ?? meta.spec}
                      </span>
                    </Stack>
                  </Stack>
                </button>
              );
            })}
          </div>
        </section>
        <section className="flex min-h-0 flex-col">
          <div className="h-8 px-3 flex items-center justify-between border-b border-[var(--color-border-subtle)] shrink-0">
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
          <div className="flex-1 overflow-y-auto p-2">
            {filtered.length === 0 ? (
              <EmptyPreflight />
            ) : (
              <Stack gap="1">
                {filtered.map((f) => (
                  <PreflightFindingRow
                    key={f.id}
                    severity={f.severity}
                    time={f.time}
                    message={f.message}
                    asset={f.asset}
                    suggestedFix={f.suggestedFix}
                    compact
                    className={cn(
                      f.message === selectedIssue?.message &&
                        f.severity === "warning" &&
                        "border-[rgba(245,158,11,0.75)] bg-[rgba(245,158,11,0.07)]",
                      (f.severity === "failure" || f.severity === "error") &&
                        "border-[rgba(239,68,68,0.65)] bg-[rgba(239,68,68,0.06)]",
                    )}
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
                <div className="mt-0.5 rounded-[var(--radius-md)] border border-[rgba(245,158,11,0.45)] bg-[rgba(245,158,11,0.08)] px-2.5 py-1.5">
                  <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">
                    Preflight complete. {counts.error + counts.failure} failure, {counts.warning} warnings found.
                  </span>
                  <p className="mt-1 text-[var(--text-caption)] text-[var(--color-text-secondary)]">
                    Address these issues to ensure the best quality and platform performance.
                  </p>
                </div>
              </Stack>
            )}
          </div>
        </section>
        <section className="grid grid-cols-[minmax(0,1fr)_248px] gap-2 border-t border-[var(--color-border-subtle)] p-2">
          <div className="relative overflow-hidden rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-black">
            <img src={podcastWide} alt="" className="absolute inset-0 h-full w-full object-cover opacity-55" />
            <div className="absolute inset-0 bg-black/30" />
            <div className="absolute inset-[10%] border border-[rgba(56,189,248,0.55)]" />
            <div className="absolute inset-y-[6%] left-[36%] right-[36%] border border-[rgba(168,85,247,0.65)] bg-[rgba(168,85,247,0.08)]" />
            <div className="absolute inset-x-[10%] bottom-[16%] h-10 border border-[rgba(245,158,11,0.7)] bg-[rgba(245,158,11,0.08)]" />
            <div className="absolute bottom-2 left-2 rounded-[var(--radius-xs)] border border-white/10 bg-black/65 px-2 py-1 text-[var(--text-caption)] text-white/75">
              Preview · safe areas
            </div>
          </div>
          <Card padding="md">
            <Stack gap="2">
              <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                Safe-area legend
              </span>
              {[
                ["YouTube 16:9", "1920x1080", "var(--color-brand-secondary)"],
                ["TikTok 9:16", "1080x1920", "var(--color-brand-purple)"],
                ["Caption safe", "mobile / TV", "var(--color-warning)"],
                ["Title safe", "1546x874", "var(--color-success)"],
              ].map(([label, value, color]) => (
                <Inline key={label} justify="between" gap="3" align="center">
                  <Inline gap="2" align="center">
                    <span className="h-2 w-2 rounded-full" style={{ backgroundColor: color }} />
                    <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)]">{label}</span>
                  </Inline>
                  <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">{value}</span>
                </Inline>
              ))}
            </Stack>
          </Card>
        </section>
      </main>

      {/* Summary */}
      <aside className="border-l border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] flex flex-col min-h-0">
        <div className="h-8 px-3 flex items-center border-b border-[var(--color-border-subtle)] shrink-0">
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
            Issue inspector
          </span>
        </div>
        <div className="flex-1 overflow-y-auto p-2.5">
          <Stack gap="3">
            {selectedIssue ? (
              <Card padding="md" tone={selectedIssue.severity === "pass" ? "default" : "warning"}>
                <Stack gap="3">
                  <Inline justify="between" align="center">
                    <span className="text-[var(--text-h3)] font-semibold text-[var(--color-text-primary)]">
                      {selectedIssue.message}
                    </span>
                    <span className="text-[var(--text-caption)] uppercase font-semibold text-[var(--color-warning)]">
                      {selectedIssue.severity}
                    </span>
                  </Inline>
                  <KV label="Asset" value={selectedIssue.asset ?? "Timeline"} />
                  {selectedIssue.time ? <KV label="Time" value={selectedIssue.time} /> : null}
                  <p className="text-[var(--text-body-sm)] leading-relaxed text-[var(--color-text-secondary)]">
                    Captions or render constraints may affect platform quality. Use the agent repair flow to apply the safest automatic fix before export.
                  </p>
                  {selectedIssue.suggestedFix ? (
                    <div className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-3 py-2 text-[var(--text-caption)] text-[var(--color-text-secondary)]">
                      {selectedIssue.suggestedFix}
                    </div>
                  ) : null}
                  <Button
                    variant="repair"
                    size="sm"
                    onClick={() => onAgentRepair?.(selectedIssue)}
                    leadingIcon={<SettingsIcon className="h-3.5 w-3.5 stroke-[1.75]" />}
                  >
                    Agent Repair
                  </Button>
                </Stack>
              </Card>
            ) : null}
            {summary ? (
              <Card padding="md">
                <Stack gap="3">
                  <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                    Render summary
                  </span>
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
            <RenderQueuePanel />
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

function DeliveryContextChip({
  label,
  tone,
}: {
  label: string;
  tone: "blue" | "green" | "purple";
}) {
  const toneClass = {
    blue: "border-[rgba(59,130,246,0.5)] bg-[rgba(59,130,246,0.12)] text-[var(--color-pill-proposed-text)]",
    green: "border-[rgba(32,201,151,0.5)] bg-[rgba(32,201,151,0.12)] text-[var(--color-pill-ready-text)]",
    purple: "border-[rgba(168,85,247,0.5)] bg-[rgba(168,85,247,0.12)] text-[var(--color-pill-reviewing-text)]",
  }[tone];

  return (
    <span
      className={cn(
        "min-w-0 truncate rounded-[var(--radius-sm)] border px-2 py-1 text-[var(--text-caption)]",
        toneClass,
      )}
    >
      {label}
    </span>
  );
}

function DeliverySuggestion({
  icon,
  title,
  detail,
  onClick,
}: {
  icon: ReactNode;
  title: string;
  detail: string;
  onClick?: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="w-full rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-3 py-2 text-left transition-colors hover:border-[var(--color-border)] hover:bg-[var(--color-surface-card-hover)]"
    >
      <Inline gap="3" align="center">
        <span className="grid h-8 w-8 shrink-0 place-items-center rounded-[var(--radius-sm)] bg-[var(--color-surface-input)] text-[var(--color-brand-secondary)] [&>svg]:h-4 [&>svg]:w-4 [&>svg]:stroke-[1.75]">
          {icon}
        </span>
        <Stack gap="0" className="min-w-0">
          <span className="truncate text-[var(--text-body-sm)] font-medium text-[var(--color-text-primary)]">
            {title}
          </span>
          <span className="truncate text-[var(--text-caption)] text-[var(--color-text-muted)]">
            {detail}
          </span>
        </Stack>
      </Inline>
    </button>
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

/** Slim left-rail replacement: target multi-select + a single
 *  "Repair with agent" button when there's a finding worth fixing.
 *  The Edit-tab CommandRail owns the agent on its tab; replicating
 *  it here added noise without adding power. */
function TargetsRail({
  resolvedTargets,
  findings,
  onToggleTarget,
  onAgentRepair,
}: {
  resolvedTargets: DeliveryTarget[];
  findings: PreflightFinding[];
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
  return (
    <div className="flex flex-col min-h-0 p-3 gap-4">
      <div>
        <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
          Targets
        </span>
        <Stack gap="1" className="mt-2">
          {resolvedTargets.map((target) => {
            const meta = TARGET_META[target.key];
            const Icon = meta.icon;
            return (
              <button
                key={target.key}
                type="button"
                onClick={() => onToggleTarget?.(target.key)}
                className={cn(
                  "flex items-center gap-2.5 rounded-[var(--radius-md)] px-2.5 py-2 text-left",
                  "border transition-colors",
                  target.active
                    ? "border-[var(--color-brand-secondary)] bg-[color-mix(in_oklab,var(--color-brand-secondary)_12%,var(--color-surface-card))]"
                    : "border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] hover:border-[var(--color-text-muted)]",
                )}
              >
                <Icon
                  className={cn(
                    "h-3.5 w-3.5 shrink-0 stroke-[1.75]",
                    target.active
                      ? "text-[var(--color-brand-secondary)]"
                      : "text-[var(--color-text-muted)]",
                  )}
                />
                <div className="min-w-0 flex-1">
                  <p className="text-[var(--text-body-sm)] font-medium text-[var(--color-text-primary)]">
                    {target.label ?? meta.label}
                  </p>
                  <p className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
                    {target.spec ?? meta.spec}
                  </p>
                </div>
                {target.active ? (
                  <span className="font-mono text-[10px] text-[var(--color-brand-secondary)]">
                    ✓
                  </span>
                ) : null}
              </button>
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
            className="mt-2 w-full rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2.5 text-left hover:border-[var(--color-text-muted)]"
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

/** Live render queue — pending, running, and recent terminal entries
 *  from `useRenderQueueStore`. Shows progress for the active render
 *  and an "Open in Finder" action on completed entries. */
function RenderQueuePanel() {
  const entries = useRenderQueueStore((s) => s.entries);
  const dismiss = useRenderQueueStore((s) => s.dismiss);
  const clearTerminal = useRenderQueueStore((s) => s.clearTerminal);
  const visible = entries.slice(-12);
  if (visible.length === 0) {
    return (
      <Card padding="md">
        <Stack gap="2">
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
            Render queue
          </span>
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
            No renders queued. Pick targets and hit Export.
          </span>
        </Stack>
      </Card>
    );
  }
  const hasTerminal = visible.some(
    (e) =>
      e.status === "done" || e.status === "failed" || e.status === "cancelled",
  );
  return (
    <Card padding="md">
      <Stack gap="3">
        <Inline justify="between" align="baseline">
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
            Render queue
          </span>
          {hasTerminal ? (
            <button
              type="button"
              onClick={clearTerminal}
              className="text-[var(--text-caption)] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]"
            >
              Clear done
            </button>
          ) : null}
        </Inline>
        <Stack gap="2">
          {visible.map((entry) => (
            <RenderQueueRow
              key={entry.id}
              entry={entry}
              onDismiss={() => dismiss(entry.id)}
            />
          ))}
        </Stack>
      </Stack>
    </Card>
  );
}

function RenderQueueRow({
  entry,
  onDismiss,
}: {
  entry: RenderQueueEntry;
  onDismiss: () => void;
}) {
  const dotClass =
    entry.status === "running"
      ? "bg-[var(--color-brand-secondary)] animate-pulse"
      : entry.status === "done"
        ? "bg-[var(--color-success)]"
        : entry.status === "failed" || entry.status === "cancelled"
          ? "bg-[#ef7168]"
          : "bg-[var(--color-text-muted)]";
  return (
    <div className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2 text-[var(--text-caption)]">
      <Inline justify="between" align="baseline" className="gap-2">
        <Inline gap="2" align="center" className="min-w-0">
          <span
            className={cn("h-1.5 w-1.5 shrink-0 rounded-full", dotClass)}
            aria-hidden
          />
          <span className="min-w-0 truncate font-medium text-[var(--color-text-primary)]">
            {entry.label}
          </span>
          <span className="shrink-0 font-mono text-[10px] uppercase tracking-[0.04em] text-[var(--color-text-muted)]">
            {entry.status}
          </span>
        </Inline>
        {entry.status === "done" || entry.status === "failed" || entry.status === "cancelled" ? (
          <button
            type="button"
            onClick={onDismiss}
            className="shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]"
            title="Dismiss"
          >
            ×
          </button>
        ) : null}
      </Inline>
      {entry.status === "running" && typeof entry.progress === "number" ? (
        <div className="mt-2 h-1 overflow-hidden rounded-full bg-[var(--color-surface-input)]">
          <div
            className="h-full rounded-full bg-[var(--color-brand-secondary)]"
            style={{ width: `${entry.progress}%` }}
          />
        </div>
      ) : null}
      {entry.status === "done" && entry.outputPath ? (
        <button
          type="button"
          onClick={() => void invokeRevealInFinder(entry.outputPath!)}
          className="mt-1 truncate text-[var(--color-brand-secondary)] hover:underline"
          title={entry.outputPath}
        >
          Open in Finder
        </button>
      ) : null}
      {entry.status === "failed" && entry.error ? (
        <p className="mt-1 truncate text-[#ef7168]" title={entry.error}>
          {entry.error}
        </p>
      ) : null}
    </div>
  );
}

async function invokeRevealInFinder(path: string): Promise<void> {
  try {
    const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
    await revealItemInDir(path);
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn("revealItemInDir failed", err);
  }
}
