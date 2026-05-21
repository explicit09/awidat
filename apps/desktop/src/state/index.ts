/**
 * Awidat v2 state stores — the orthogonal nav axes from the design spec.
 *
 * - useStageStore: Intent → Indexing → Proposal → Review → Revise → Deliver
 * - useLensStore:  Import / Index / Selects / Assembly / Review / Captions / Audio / Color / Delivery
 *
 * Stages = state (where in the work). Lenses = navigation (which view).
 * They are NOT a single nav tree — a user can be in any stage viewing any lens.
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
  useLensStore,
  LENSES,
  LENS_LABEL,
  type Lens,
  type LensStore,
} from "./lenses";
