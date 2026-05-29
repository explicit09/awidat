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
  stageProgress,
  type Stage,
  type StageProgress,
  type StageStore,
} from "./stages";

export {
  useSkillsStore,
  type SkillsStore,
} from "./skills";

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
