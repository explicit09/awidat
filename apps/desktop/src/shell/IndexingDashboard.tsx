import {
  AudioWaveform,
  Captions,
  Clapperboard,
  Database,
  ExternalLink,
  FileText,
  Gauge,
  HardDrive,
  Import,
  LocateFixed,
  Palette,
  Scan,
  Sparkles,
  Thermometer,
  Users,
  VolumeX,
  type LucideIcon,
} from "lucide-react";
import type { ReactNode } from "react";
import {
  Button,
  Card,
  Inline,
  MediaStatusRow,
  Pill,
  Stack,
  cn,
  type MediaIndexingStatus,
} from "../ui";

/**
 * IndexingDashboard — concept Screen 6.
 *
 * Unified indexing surface: project context header, media list, the
 * 9-row indexing pipeline (with per-task status), system-status footer
 * (local processing / disk / temperature), and the hand-off CTA
 * "Ask agent for first cut" that routes to the Proposal stage.
 */

export type IndexingMediaItem = {
  id: string;
  title: string;
  detail?: string;
  status: MediaIndexingStatus;
  progress?: number;
  thumbnail?: string;
};

export type IndexingTask = {
  id: string;
  /** One of the 9 named tasks from the design spec. */
  kind:
    | "transcripts"
    | "scenes"
    | "audio"
    | "face"
    | "motion"
    | "color"
    | "silence"
    | "speaker"
    | "captions";
  status: MediaIndexingStatus;
  progress?: number;
  detail?: string;
};

export type IndexingSystemStatus = {
  cpu?: number;
  diskFreeGB?: number;
  tempC?: number;
};

export type IndexerConfigEntry = {
  name: string;
  enabled: boolean;
  command: string;
  args: string[];
  cwd?: string | null;
  dependsOn: string[];
  resourceClass: string;
  group?: string | null;
  userConfigured: boolean;
};

export type IndexerConfigSnapshot = {
  globalPath?: string | null;
  projectPath?: string | null;
  indexers: IndexerConfigEntry[];
};

export type IndexingDashboardProps = {
  projectName?: string;
  sourceCount?: number;
  deliveryTarget?: string;
  media?: IndexingMediaItem[];
  tasks?: IndexingTask[];
  system?: IndexingSystemStatus;
  indexerConfig?: IndexerConfigSnapshot;
  /** True when all 9 tasks are at least partially indexed. */
  ready?: boolean;
  onImport?: () => void;
  onImportUrl?: () => void;
  onAskAgent?: () => void;
  onReviewIndexResults?: () => void;
  onToggleIndexer?: (indexer: IndexerConfigEntry) => void;
  onOpenConfigPath?: (path: string) => void;
  onRevealConfigPath?: (path: string) => void;
  showImportActions?: boolean;
};

const TASK_META: Record<
  IndexingTask["kind"],
  { icon: LucideIcon; label: string }
> = {
  transcripts: { icon: FileText, label: "Transcripts" },
  scenes: { icon: Clapperboard, label: "Scenes" },
  audio: { icon: AudioWaveform, label: "Audio analysis" },
  face: { icon: Scan, label: "Face detection" },
  motion: { icon: Gauge, label: "Motion analysis" },
  color: { icon: Palette, label: "Color analysis" },
  silence: { icon: VolumeX, label: "Silence detection" },
  speaker: { icon: Users, label: "Speaker diarization" },
  captions: { icon: Captions, label: "Caption readiness" },
};

const ALL_TASK_KINDS: IndexingTask["kind"][] = [
  "transcripts",
  "scenes",
  "audio",
  "face",
  "motion",
  "color",
  "silence",
  "speaker",
  "captions",
];

export function IndexingDashboard({
  projectName,
  sourceCount = 0,
  deliveryTarget,
  media = [],
  tasks = [],
  system,
  indexerConfig,
  ready = false,
  onImport,
  onImportUrl,
  onAskAgent,
  onReviewIndexResults,
  onToggleIndexer,
  onOpenConfigPath,
  onRevealConfigPath,
  showImportActions = true,
}: IndexingDashboardProps) {
  // Resolve each of the 9 named tasks — fill missing ones with status="missing"
  // so the UI always shows all 9 rows.
  const resolved: IndexingTask[] = ALL_TASK_KINDS.map((kind) => {
    const provided = tasks.find((t) => t.kind === kind);
    return (
      provided ?? {
        id: `missing-${kind}`,
        kind,
        status: "missing" as const,
      }
    );
  });
  const indexedTaskCount = resolved.filter((task) => task.status !== "missing").length;
  const overallProgress = Math.round(
    resolved.reduce((sum, task) => {
      if (typeof task.progress === "number") return sum + task.progress;
      if (task.status === "indexed" || task.status === "imported") return sum + 100;
      if (task.status === "processing") return sum + 45;
      if (task.status === "partial") return sum + 50;
      return sum;
    }, 0) / resolved.length,
  );

  return (
    <div className="grid h-full min-h-0 grid-cols-[minmax(250px,0.82fr)_minmax(300px,1fr)_minmax(390px,1.68fr)_minmax(220px,0.78fr)] bg-[var(--color-surface-app)]">
      <aside className="border-r border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] flex flex-col min-h-0">
        <div className="px-4 py-4 border-b border-[var(--color-border-subtle)]">
          <Stack gap="3">
            <Stack gap="1">
              <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                Project & import
              </span>
              <span className="text-[var(--text-h3)] font-semibold text-[var(--color-text-primary)]">
                {projectName ?? "Untitled"}
              </span>
              <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
                {sourceCount} {sourceCount === 1 ? "file" : "files"} · {media.length > 0 ? "12.4 GB" : "No media imported"}
              </span>
            </Stack>
            {deliveryTarget ? (
              <Pill status="proposed" dot={false}>
                {deliveryTarget}
              </Pill>
            ) : null}
            {showImportActions ? (
              <Inline gap="2">
                <Button
                  variant="primary"
                  size="sm"
                  onClick={onImport}
                  leadingIcon={<Import className="h-3.5 w-3.5 stroke-[1.75]" />}
                  className="flex-1"
                >
                  Import files
                </Button>
                <Button variant="secondary" size="sm" onClick={onImportUrl} className="flex-1">
                  Import URL
                </Button>
              </Inline>
            ) : null}
          </Stack>
        </div>
        {showImportActions ? (
          <div className="p-4 border-b border-[var(--color-border-subtle)]">
            <DropZone onImport={onImport} />
          </div>
        ) : null}
        <div className="flex-1 overflow-y-auto p-4">
          <Stack gap="3">
            <SectionHeader title="Import status" subtitle="Complete" />
            <ProgressStat label="Importing 9 of 9 files" value="12.4 GB / 12.4 GB" progress={100} />
            <SectionHeader title="Activity log" />
            {[
              "Scanned folder and matched 9 supported files",
              "Generated proxies for A and B camera",
              "Queued transcript, scene, and speaker analysis",
              "Audio waveform cache is ready",
            ].map((item, index) => (
              <div key={item} className="grid grid-cols-[24px_1fr] gap-2 text-[var(--text-caption)] text-[var(--color-text-secondary)]">
                <span className="font-mono text-[var(--color-text-muted)]">{String(index + 1).padStart(2, "0")}</span>
                <span className="leading-snug">{item}</span>
              </div>
            ))}
          </Stack>
        </div>
      </aside>

      <section className="grid min-h-0 grid-rows-[1fr_auto] border-r border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)]">
        <Section title={`Imported media (${media.length})`} subtitle="Local sources" fill>
          {media.length === 0 ? (
            <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">No media yet.</span>
          ) : (
            <Stack gap="2">
              {media.map((m) => (
                <MediaStatusRow
                  key={m.id}
                  title={m.title}
                  detail={m.detail}
                  status={m.status}
                  progress={m.progress}
                  thumbnail={m.thumbnail}
                />
              ))}
            </Stack>
          )}
        </Section>
        <div className="border-t border-[var(--color-border-subtle)] px-4 py-2">
          <Inline justify="between" align="center">
            <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
              {media.length} items · 12.4 GB
            </span>
            <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
              Indexed locally
            </span>
          </Inline>
        </div>
      </section>

      <main className="grid min-h-0 grid-rows-[minmax(0,1fr)_218px] overflow-hidden">
        <Section
          title="Indexing pipeline"
          subtitle={
            ready
              ? "All signals ready"
              : `${indexedTaskCount} of 9`
          }
        >
          <Stack gap="2">
            <Inline justify="between" align="baseline">
              <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
                Overall progress
              </span>
              <span className="font-mono text-[var(--text-caption)] text-[var(--color-brand-secondary-hover)]">
                {overallProgress}%
              </span>
            </Inline>
            <div className="h-1.5 overflow-hidden rounded-full bg-[var(--color-surface-input)]">
              <div
                className="h-full rounded-full bg-[var(--color-brand-secondary)]"
                style={{ width: `${overallProgress}%` }}
              />
            </div>
          </Stack>
          <div className="grid grid-cols-1 gap-2">
            {resolved.map((task) => {
              const meta = TASK_META[task.kind];
              const Icon = meta.icon;
              return (
                <MediaStatusRow
                  key={task.id}
                  title={meta.label}
                  detail={task.detail ?? statusDetail(task.status)}
                  status={task.status}
                  progress={task.progress}
                  icon={<Icon className="h-4 w-4 stroke-[1.75]" />}
                  className="min-h-[58px]"
                />
              );
            })}
          </div>
        </Section>

        <section className="border-t border-[var(--color-border-subtle)] px-5 py-4 bg-[var(--color-surface-panel)]">
          <Stack gap="3">
            <SectionHeader title="Extracted structure preview" subtitle="Updating live" />
            <div className="grid grid-cols-3 gap-2 xl:grid-cols-5">
              <MetricTile label="Duration" value="00:42:11" />
              <MetricTile label="Scenes" value="368" />
              <MetricTile label="Segments" value="126" />
              <MetricTile label="Speakers" value="2" />
              <MetricTile label="Transcript" value="78%" />
            </div>
            <div className="grid grid-cols-[1fr_140px] gap-3">
              <div className="flex h-10 overflow-hidden rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)]">
                {Array.from({ length: 14 }).map((_, index) => (
                  <div
                    key={index}
                    className="border-r border-black/40"
                    style={{
                      width: `${index % 3 === 0 ? 9 : index % 2 === 0 ? 6 : 8}%`,
                      background: index % 4 === 0 ? "rgba(56,189,248,0.38)" : index % 3 === 0 ? "rgba(168,85,247,0.34)" : "rgba(100,116,139,0.28)",
                    }}
                  />
                ))}
              </div>
              <span className="self-center text-[var(--text-caption)] text-[var(--color-text-muted)]">+363 scene markers</span>
            </div>
          </Stack>
        </section>
      </main>

      <aside className="border-l border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] flex flex-col min-h-0">
        <div className="flex-1 overflow-y-auto p-4">
          <Stack gap="4">
            <Card
              tone={ready ? "accent" : "default"}
              padding="lg"
            >
              <Stack gap="3">
                <Inline gap="2" align="center">
                  <Sparkles
                    className={cn(
                      "h-5 w-5 stroke-[1.5]",
                      ready ? "text-[var(--color-brand)]" : "text-[var(--color-text-muted)]",
                    )}
                  />
                  <span className="text-[var(--text-h3)] font-semibold text-[var(--color-text-primary)]">
                    {ready ? "Ready for the first cut" : "Indexing in progress"}
                  </span>
                </Inline>
                <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)] leading-relaxed">
                  {ready
                    ? "Awidat has indexed enough signals to start proposing edits. Hand off to the agent and review what it suggests."
                    : "The agent works best with the transcript and a few signals at minimum. You can ask now and the agent will pick up more signals as they finish."}
                </span>
                <Button
                  variant={ready ? "primary" : "secondary"}
                  onClick={onAskAgent}
                  trailingIcon={<Sparkles className="h-3.5 w-3.5 stroke-[1.75]" />}
                >
                  Ask agent for first cut
                </Button>
                <Button variant="ghost" size="sm" onClick={onReviewIndexResults}>
                  Review index results
                </Button>
              </Stack>
            </Card>

            <Card padding="md">
              <Stack gap="3">
                <SectionHeader title="Smart hints" />
                {[
                  ["2 segments may benefit from tighter cuts", "Review now"],
                  ["Audio levels vary across 3% of the timeline", "View in Audio"],
                  ["Faces detected in 95% of the timeline", "Good coverage"],
                ].map(([label, value]) => (
                  <Inline key={label} justify="between" gap="3" align="baseline">
                    <span className="text-[var(--text-caption)] leading-snug text-[var(--color-text-secondary)]">{label}</span>
                    <span className="shrink-0 text-[var(--text-caption)] font-semibold text-[var(--color-brand-secondary-hover)]">{value}</span>
                  </Inline>
                ))}
              </Stack>
            </Card>

            {indexerConfig ? (
              <Card padding="md">
                <Stack gap="3">
                  <SectionHeader
                    title="Indexer config"
                    subtitle={`${indexerConfig.indexers.filter((indexer) => indexer.enabled).length} active`}
                  />
                  <Stack gap="1" className="max-h-[236px] overflow-y-auto pr-1">
                    {indexerConfig.indexers.map((indexer) => (
                      <IndexerConfigRow
                        key={indexer.name}
                        indexer={indexer}
                        onToggle={onToggleIndexer}
                      />
                    ))}
                  </Stack>
                  <Stack gap="1">
                    <ConfigPath
                      label="Project"
                      value={indexerConfig.projectPath}
                      onOpen={onOpenConfigPath}
                      onReveal={onRevealConfigPath}
                    />
                    <ConfigPath
                      label="Global"
                      value={indexerConfig.globalPath}
                      onOpen={onOpenConfigPath}
                      onReveal={onRevealConfigPath}
                    />
                  </Stack>
                </Stack>
              </Card>
            ) : null}

            {system ? (
              <Card padding="md">
                <Stack gap="3">
                  <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                    System
                  </span>
                  <SystemRow
                    icon={<Gauge />}
                    label="CPU"
                    value={typeof system.cpu === "number" ? `${Math.round(system.cpu)}%` : "—"}
                    bar={typeof system.cpu === "number" ? system.cpu / 100 : undefined}
                  />
                  <SystemRow
                    icon={<HardDrive />}
                    label="Disk free"
                    value={
                      typeof system.diskFreeGB === "number"
                        ? `${system.diskFreeGB.toFixed(1)} GB`
                        : "—"
                    }
                  />
                  <SystemRow
                    icon={<Thermometer />}
                    label="Temp"
                    value={
                      typeof system.tempC === "number"
                        ? `${system.tempC.toFixed(0)}°C`
                        : "—"
                    }
                  />
                </Stack>
              </Card>
            ) : null}
          </Stack>
        </div>
      </aside>
    </div>
  );
}

function IndexerConfigRow({
  indexer,
  onToggle,
}: {
  indexer: IndexerConfigEntry;
  onToggle?: (indexer: IndexerConfigEntry) => void;
}) {
  return (
    <div className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2.5 py-2">
      <Stack gap="1">
        <Inline justify="between" gap="2" align="center">
          <span className="min-w-0 truncate text-[var(--text-body-sm)] font-semibold text-[var(--color-text-primary)]">
            {indexer.name}
          </span>
          <Inline gap="1" align="center" className="shrink-0">
            {indexer.userConfigured ? (
              <Pill status="proposed" dot={false}>override</Pill>
            ) : null}
            <Pill status={indexer.enabled ? "ready" : "missing"} dot={false}>
              {indexer.enabled ? "on" : "off"}
            </Pill>
            {onToggle ? (
              <button
                type="button"
                onClick={() => onToggle(indexer)}
                className="h-5 rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-1.5 text-[var(--text-caption)] font-semibold text-[var(--color-text-secondary)] transition-colors hover:border-[var(--color-border)] hover:text-[var(--color-text-primary)]"
              >
                {indexer.enabled ? "Disable" : "Enable"}
              </button>
            ) : null}
          </Inline>
        </Inline>
        <Inline justify="between" gap="2" align="baseline">
          <span className="min-w-0 truncate font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
            {indexer.command} {indexer.args.join(" ")}
          </span>
          <span className="shrink-0 text-[var(--text-caption)] text-[var(--color-text-muted)]">
            {indexer.group ?? indexer.resourceClass}
          </span>
        </Inline>
        {indexer.dependsOn.length > 0 ? (
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
            Depends on {indexer.dependsOn.join(", ")}
          </span>
        ) : null}
      </Stack>
    </div>
  );
}

function ConfigPath({
  label,
  value,
  onOpen,
  onReveal,
}: {
  label: string;
  value?: string | null;
  onOpen?: (path: string) => void;
  onReveal?: (path: string) => void;
}) {
  const canAct = Boolean(value);
  return (
    <Inline justify="between" gap="2" align="center">
      <span className="shrink-0 text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        {label}
      </span>
      <span className="min-w-0 truncate font-mono text-[var(--text-caption)] text-[var(--color-text-secondary)]">
        {value ?? "not available"}
      </span>
      <Inline gap="1" align="center" className="shrink-0">
        <button
          type="button"
          disabled={!canAct || !onOpen}
          onClick={() => value && onOpen?.(value)}
          className="grid h-5 w-5 place-items-center rounded-[var(--radius-xs)] text-[var(--color-text-muted)] transition-colors hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)] disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-[var(--color-text-muted)]"
          aria-label={`Open ${label.toLowerCase()} config`}
          title={`Open ${label.toLowerCase()} config`}
        >
          <ExternalLink className="h-3 w-3 stroke-[1.75]" />
        </button>
        <button
          type="button"
          disabled={!canAct || !onReveal}
          onClick={() => value && onReveal?.(value)}
          className="grid h-5 w-5 place-items-center rounded-[var(--radius-xs)] text-[var(--color-text-muted)] transition-colors hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)] disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-[var(--color-text-muted)]"
          aria-label={`Reveal ${label.toLowerCase()} config`}
          title={`Reveal ${label.toLowerCase()} config`}
        >
          <LocateFixed className="h-3 w-3 stroke-[1.75]" />
        </button>
      </Inline>
    </Inline>
  );
}

function SectionHeader({ title, subtitle }: { title: string; subtitle?: string }) {
  return (
    <Inline justify="between" align="baseline">
      <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        {title}
      </span>
      {subtitle ? (
        <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">{subtitle}</span>
      ) : null}
    </Inline>
  );
}

function ProgressStat({ label, value, progress }: { label: string; value: string; progress: number }) {
  return (
    <Stack gap="2">
      <Inline justify="between" align="baseline">
        <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">{label}</span>
        <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">{value}</span>
      </Inline>
      <div className="h-1.5 w-full overflow-hidden rounded-full bg-[var(--color-surface-input)]">
        <div className="h-full rounded-full bg-[var(--color-brand)]" style={{ width: `${Math.max(0, Math.min(100, progress))}%` }} />
      </div>
    </Stack>
  );
}

function MetricTile({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2.5 py-2">
      <div className="font-mono text-[var(--text-body-sm)] font-semibold text-[var(--color-text-primary)]">{value}</div>
      <div className="text-[var(--text-caption)] text-[var(--color-text-muted)]">{label}</div>
    </div>
  );
}

function statusDetail(status: MediaIndexingStatus): string {
  switch (status) {
    case "missing":
      return "Pending — not yet computed";
    case "indexing":
      return "Computing now";
    case "indexed":
      return "Indexed";
    case "imported":
      return "Imported";
    case "processing":
      return "Processing";
    case "partial":
      return "Partial";
    case "failed":
      return "Failed";
  }
}

function Section({
  title,
  subtitle,
  children,
  fill = false,
}: {
  title: string;
  subtitle?: string;
  children: ReactNode;
  fill?: boolean;
}) {
  return (
    <div
      className={cn(
        "min-h-0 overflow-y-auto px-5 py-4 border-b border-[var(--color-border-subtle)]",
        fill ? "h-full" : "h-full",
      )}
    >
      <Stack gap="3">
        <SectionHeader title={title} subtitle={subtitle} />
        {children}
      </Stack>
    </div>
  );
}

function DropZone({ onImport }: { onImport?: () => void }) {
  return (
    <button
      type="button"
      onClick={onImport}
      className="w-full rounded-[var(--radius-lg)] border-2 border-dashed border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] hover:border-[var(--color-border)] hover:bg-[var(--color-surface-card-hover)] transition-colors py-12 flex flex-col items-center gap-2"
    >
      <Database className="h-8 w-8 stroke-[1.5] text-[var(--color-text-muted)]" />
      <span className="text-[var(--text-body-sm)] font-medium text-[var(--color-text-primary)]">
        Drop media here or click to import
      </span>
      <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
        MP4, MOV, MXF, WAV, MP3, SRT, VTT
      </span>
    </button>
  );
}

function SystemRow({
  icon,
  label,
  value,
  bar,
}: {
  icon: ReactNode;
  label: string;
  value: string;
  bar?: number;
}) {
  return (
    <Stack gap="1">
      <Inline justify="between" align="center">
        <Inline gap="2" align="center">
          <span className="text-[var(--color-text-muted)] [&>svg]:h-3.5 [&>svg]:w-3.5 [&>svg]:stroke-[1.75]">
            {icon}
          </span>
          <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">{label}</span>
        </Inline>
        <span className="font-mono text-[var(--text-body-sm)] text-[var(--color-text-primary)]">{value}</span>
      </Inline>
      {typeof bar === "number" ? (
        <div className="h-1 w-full overflow-hidden rounded-full bg-[var(--color-surface-input)]">
          <div
            className="h-full rounded-full bg-[var(--color-brand-secondary)]"
            style={{ width: `${Math.max(0, Math.min(100, bar * 100))}%` }}
          />
        </div>
      ) : null}
    </Stack>
  );
}
