/**
 * Stages — the two human-facing workflow destinations:
 *   Edit → Deliver.
 *
 * Intent is handled by the command rail. Indexing, proposal, review, and revise
 * are panels inside the Edit workspace instead of standalone destinations.
 */

import { create } from "zustand";

/**
 * Linear workflow milestones — these get iterated by chrome that
 * draws a left-to-right "progress" indicator. Skills is intentionally
 * NOT in this list: it's a non-linear destination (a tab the user
 * jumps to and back from), not a milestone in the edit → deliver path.
 */
export const STAGES = ["edit", "deliver"] as const;

/**
 * The full set of routable destinations. `Stage` is the workflow
 * surface union; `skills` is a non-linear tab that lives next to the
 * workflow stages in the WorkspaceRow but doesn't participate in the
 * progress indicator.
 */
export type Stage = (typeof STAGES)[number] | "skills";

export const STAGE_LABEL: Record<Stage, string> = {
  edit: "Edit",
  deliver: "Deliver",
  skills: "Skills",
};

/**
 * For the StageIndicator chrome: each stage can be at one of these visual states.
 */
export type StageProgress = "upcoming" | "current" | "complete" | "blocked";

export type StageStore = {
  current: Stage;
  /** Stages the user has reached at least once in this project. Used to render `complete` chips. */
  visited: Set<Stage>;
  set: (stage: Stage) => void;
  reset: () => void;
};

export const useStageStore = create<StageStore>((set) => ({
  current: "edit",
  visited: new Set<Stage>(["edit"]),
  set: (stage) =>
    set((s) => ({
      current: stage,
      visited: new Set([...s.visited, stage]),
    })),
  reset: () => set({ current: "edit", visited: new Set<Stage>(["edit"]) }),
}));

/**
 * Derives the visual progress state for a given stage relative to `current`.
 * Used by StageIndicator (Phase 2.3).
 */
export function stageProgress(stage: Stage, current: Stage, visited: Set<Stage>): StageProgress {
  if (stage === current) return "current";
  if (visited.has(stage)) return "complete";
  return "upcoming";
}
