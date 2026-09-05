import type { MediaIndexingStatus } from "../ui";

// Shared media and indexer data used by the workspace.

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
