import {
  AudioWaveform,
  Captions,
  Clapperboard,
  Database,
  FileText,
  Gauge,
  HardDrive,
  Import,
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

export type IndexingDashboardProps = {
  projectName?: string;
  sourceCount?: number;
  deliveryTarget?: string;
  media?: IndexingMediaItem[];
  tasks?: IndexingTask[];
  system?: IndexingSystemStatus;
  /** True when all 9 tasks are at least partially indexed. */
  ready?: boolean;
  onImport?: () => void;
  onImportUrl?: () => void;
  onAskAgent?: () => void;
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
  ready = false,
  onImport,
  onImportUrl,
  onAskAgent,
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

  return (
    <div className="grid h-full grid-cols-[1fr_360px] bg-[var(--color-surface-app)] min-h-0">
      {/* Left: media + pipeline */}
      <div className="flex flex-col min-h-0 overflow-y-auto">
        {/* Header */}
        <div className="px-6 py-4 border-b border-[var(--color-border-subtle)]">
          <Inline justify="between" align="end">
            <Stack gap="1">
              <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                Project
              </span>
              <span className="text-[var(--text-h2)] font-semibold text-[var(--color-text-primary)]">
                {projectName ?? "Untitled"}
              </span>
              <Inline gap="3" align="center">
                <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
                  {sourceCount} {sourceCount === 1 ? "source" : "sources"}
                </span>
                {deliveryTarget ? (
                  <Pill status="proposed" dot={false}>
                    {deliveryTarget}
                  </Pill>
                ) : null}
              </Inline>
            </Stack>
            <Inline gap="2">
              <Button variant="secondary" size="sm" onClick={onImportUrl}>
                Import from URL
              </Button>
              <Button
                variant="primary"
                size="sm"
                onClick={onImport}
                leadingIcon={<Import className="h-3.5 w-3.5 stroke-[1.75]" />}
              >
                Import files
              </Button>
            </Inline>
          </Inline>
        </div>

        {/* Media list */}
        <Section title="Media" subtitle={`${media.length} items`}>
          {media.length === 0 ? (
            <DropZone onImport={onImport} />
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

        {/* Indexing pipeline */}
        <Section
          title="Indexing pipeline"
          subtitle={
            ready
              ? "All signals ready"
              : `${resolved.filter((t) => t.status !== "missing").length} of 9`
          }
        >
          <Stack gap="2">
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
                />
              );
            })}
          </Stack>
        </Section>
      </div>

      {/* Right: ready CTA + system status */}
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
              </Stack>
            </Card>

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
}: {
  title: string;
  subtitle?: string;
  children: ReactNode;
}) {
  return (
    <div className="px-6 py-4 border-b border-[var(--color-border-subtle)]">
      <Inline justify="between" align="baseline" className="mb-3">
        <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
          {title}
        </span>
        {subtitle ? (
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
            {subtitle}
          </span>
        ) : null}
      </Inline>
      {children}
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
        mp4 / mov / mkv · audio / video
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
