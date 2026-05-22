/**
 * Stages — the three human-facing workflow destinations:
 *   Indexing → Edit → Deliver.
 *
 * Intent is handled by the command rail, and proposal/review/revise are one edit loop.
 */

import { create } from "zustand";

export const STAGES = ["indexing", "edit", "deliver"] as const;
export type Stage = (typeof STAGES)[number];

export const STAGE_LABEL: Record<Stage, string> = {
  indexing: "Index",
  edit: "Edit",
  deliver: "Deliver",
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
  current: "indexing",
  visited: new Set<Stage>(["indexing"]),
  set: (stage) =>
    set((s) => ({
      current: stage,
      visited: new Set([...s.visited, stage]),
    })),
  reset: () => set({ current: "indexing", visited: new Set<Stage>(["indexing"]) }),
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
