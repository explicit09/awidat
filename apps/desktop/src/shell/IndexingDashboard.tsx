import type { MediaIndexingStatus } from "../ui";

/**
 * Compatibility shim — the 776-line dashboard concept was split into
 * `IndexRailPro` (the Pro-mode dense rail) and a Creator surface that
 * arrives in Task 15. Existing callers that imported `IndexingDashboard`
 * still receive a render via the re-export below; the public type contract
 * (props, media items, tasks, etc.) is preserved in this file so downstream
 * code keeps compiling unchanged.
 *
 * New callers should import directly from `./IndexRail` and pass only the
 * subset of props the rail actually consumes.
 */

export type IndexingMediaItem = {
  id: string;
  assetId?: string;
  title: string;
  stem?: string;
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

export type IndexingStructurePreview = {
  duration?: string;
  scenes?: number;
  segments?: number;
  speakers?: number;
  transcriptPercent?: number;
};

export type IndexingEpisodeSummary = {
  total: number;
  accepted: number;
  reviewNeeded: number;
  rejected: number;
  episodes: Array<{
    id: string;
    name: string;
    order: number;
    startS: number;
    endS: number;
    durationS: number;
    confidence: number;
    status: "accepted" | "review_needed" | "rejected";
    evidenceCount?: number;
  }>;
};

export type IndexingDashboardProps = {
  projectName?: string;
  sourceCount?: number;
  deliveryTarget?: string;
  media?: IndexingMediaItem[];
  tasks?: IndexingTask[];
  system?: IndexingSystemStatus;
  indexerConfig?: IndexerConfigSnapshot;
  structurePreview?: IndexingStructurePreview;
  /**
   * Origin's first-class episodes feature (parallel to our Wave 3
   * evidence drill-downs). Preserved as an optional prop so origin's
   * App.tsx callers compile; the shim's IndexRail render does not yet
   * surface this — follow-up to wire it as an evidence drill-down or
   * dedicated episode panel.
   */
  episodes?: IndexingEpisodeSummary;
  /**
   * Raw one-line status of the currently-running indexer job, e.g.
   * `"running face · raw/Demo.mp4 · 14 / 84 pairs complete"`. The
   * rail parses out the indexer name + asset for the meta line.
   * Undefined / empty when no indexing job is in flight.
   */
  activeIndexingStatus?: string;
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


export { IndexRail as IndexingDashboard } from "./IndexRail";
