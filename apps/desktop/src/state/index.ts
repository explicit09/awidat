/**
 * Awidat v2 state stores.
 *
 * - useStageStore: Edit → Deliver
 *
 * Stage is the single global workflow navigation surface. Specialist review
 * controls live inside their workspace instead of a second global nav row.
 */

export {
  useStageStore,
  STAGES,
  STAGE_LABEL,
  WORKSPACE_DESTINATIONS,
  WORKSPACE_SHORTCUTS,
  stageFromWorkspaceShortcut,
  stageProgress,
  type Stage,
  type StageProgress,
  type StageStore,
  type WorkspaceShortcut,
} from "./stages";

export {
  useSkillsStore,
  type SkillsStore,
  type PinnedSkill,
} from "./skills";

export {
  useIndexerOverlay,
  type IndexerOverlayStore,
} from "./indexerOverlay";

export {
  useProposalHistoryStore,
  buildHistoryEntry,
  entriesForProject,
  sortNewestFirst,
  serialize as serializeHistory,
  deserialize as deserializeHistory,
  type HistoryEntry,
  type HistoryDecision,
} from "./proposalHistory";
