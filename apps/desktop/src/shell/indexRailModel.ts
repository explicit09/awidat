import type { JobPillState } from "../ui/primitives/StatusPill";
import type {
  IndexerConfigEntry,
  IndexerConfigSnapshot,
  IndexingStructurePreview,
  IndexingTask,
} from "./IndexingDashboard";
import type { MediaIndexingStatus } from "../ui";

/**
 * Pure model builder for IndexRailPro — flattens the rich IndexingDashboard
 * task / config / preview shape into the dense rail's view model.
 *
 * Lives in a `.ts` so the mapping logic is testable without a JSX
 * transformer if the rail ever needs unit coverage.
 */

export interface RailSignal {
  name: string;
  state: JobPillState;
  percent?: number;
}

export interface RailModel {
  percent: number;
  ready: number;
  total: number;
  queued: number;
  etaText?: string;
  durationLabel?: string;
  scenes?: number;
  segments?: number;
  transcriptLabel?: string;
  bySection: { speech: RailSignal[]; visuals: RailSignal[]; audio: RailSignal[] };
  indexers: IndexerConfigEntry[];
}

export const ALL_TASK_KINDS: IndexingTask["kind"][] = [
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

const TASK_LABEL: Record<IndexingTask["kind"], string> = {
  transcripts: "Transcript",
  scenes: "Scenes",
  audio: "Audio",
  face: "Face",
  motion: "Motion",
  color: "Color",
  silence: "Silence",
  speaker: "Speaker",
  captions: "Captions",
};

const SECTION_FOR: Record<IndexingTask["kind"], "speech" | "visuals" | "audio"> = {
  transcripts: "speech",
  speaker: "speech",
  captions: "speech",
  scenes: "visuals",
  face: "visuals",
  motion: "visuals",
  color: "visuals",
  audio: "audio",
  silence: "audio",
};

/**
 * Map MediaIndexingStatus to the 4-state JobPillState the StatusPill renders.
 * Mirrors MediaStatusRow's PILL_FOR but flattens "proposal" pills into
 * "running" since IndexRailPro doesn't model human-review states.
 */
export function statusToJobState(status: MediaIndexingStatus): JobPillState {
  switch (status) {
    case "indexed":
    case "imported":
      return "ready";
    case "indexing":
    case "processing":
    case "queued":
    case "partial":
      return "running";
    case "failed":
      return "failed";
    case "missing":
    default:
      return "idle";
  }
}

export function progressForTask(task: IndexingTask): number {
  if (typeof task.progress === "number") return task.progress;
  if (task.status === "indexed" || task.status === "imported") return 100;
  if (task.status === "processing") return 45;
  if (task.status === "partial") return 50;
  return 0;
}

export function buildRailModel(
  tasks: IndexingTask[],
  preview?: IndexingStructurePreview,
  config?: IndexerConfigSnapshot,
): RailModel {
  const resolved: IndexingTask[] = ALL_TASK_KINDS.map((kind) => {
    const provided = tasks.find((t) => t.kind === kind);
    return provided ?? { id: `missing-${kind}`, kind, status: "missing" as const };
  });

  const signals: RailSignal[] = resolved.map((task) => {
    const state = statusToJobState(task.status);
    const percent = progressForTask(task);
    return {
      name: TASK_LABEL[task.kind],
      state,
      percent: state === "running" ? percent : undefined,
    };
  });

  const bySection: RailModel["bySection"] = { speech: [], visuals: [], audio: [] };
  resolved.forEach((task, i) => {
    bySection[SECTION_FOR[task.kind]].push(signals[i]);
  });

  const total = resolved.length;
  const ready = resolved.filter((t) => t.status === "indexed" || t.status === "imported").length;
  const queued = resolved.filter(
    (t) =>
      t.status === "queued" ||
      t.status === "indexing" ||
      t.status === "processing" ||
      t.status === "partial",
  ).length;
  const sumPercent = resolved.reduce((acc, t) => acc + progressForTask(t), 0);
  const percent = Math.round(sumPercent / total);

  const transcriptLabel =
    typeof preview?.transcriptPercent === "number" ? `${preview.transcriptPercent}%` : undefined;

  return {
    percent,
    ready,
    total,
    queued,
    etaText: undefined,
    durationLabel: preview?.duration,
    scenes: preview?.scenes,
    segments: preview?.segments,
    transcriptLabel,
    bySection,
    indexers: config?.indexers ?? [],
  };
}
