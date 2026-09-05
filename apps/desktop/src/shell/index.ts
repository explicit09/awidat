export { StageShell, type StageShellProps } from "./StageShell";
export type { ContextChip, ChatSessionSummary } from "../agent/turnContext";
export type { MediaSuggestion } from "./StageConversation";
export { PreviewInsights, type PreviewChange } from "./PreviewInsights";
export {
  ProposalInspector,
  type ProposalInspectorProps,
  type ProposalInspectorData,
  type EvidenceItem,
  type EvidenceKind,
  type Alternative,
} from "./ProposalInspector";
export {
  type IndexingMediaItem,
  type IndexingTask,
  type IndexingStructurePreview,
  type IndexingEpisodeSummary,
  type IndexerConfigSnapshot,
  type IndexerConfigEntry,
} from "./indexRailTypes";
export { IndexRail, type IndexRailProps } from "./IndexRail";
export {
  DeliverySurface,
  DRAFT_METADATA_JOB_ID,
  type DeliverySurfaceProps,
  type DeliveryTarget,
  type DeliveryTargetKey,
  type PreflightFinding,
  type DeliveryRenderSummary,
} from "./DeliverySurface";
export { SkillsSurface, type SkillEntry } from "./SkillsSurface";
export { HistorySurface } from "./HistorySurface";
