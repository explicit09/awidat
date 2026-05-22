/**
 * Stages — the six-step human-in-the-loop loop from the design spec:
 *   Intent → Indexing → Proposal → Review → Revise → Deliver.
 *
 * The stage represents *where in the work* the user is. It is state, not navigation
 * (the user does not freely jump between stages; the agent advances them, and the
 * user reviews/corrects within a stage).
 *
 * This is one of the two orthogonal nav axes — see ./lenses.ts for the other.
 */

import { create } from "zustand";

export const STAGES = ["intent", "indexing", "proposal", "review", "revise", "deliver"] as const;
export type Stage = (typeof STAGES)[number];

export const STAGE_LABEL: Record<Stage, string> = {
  intent: "Intent",
  indexing: "Indexing",
  proposal: "Proposal",
  review: "Review",
  revise: "Revise",
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
  current: "intent",
  visited: new Set<Stage>(["intent"]),
  set: (stage) =>
    set((s) => ({
      current: stage,
      visited: new Set([...s.visited, stage]),
    })),
  reset: () => set({ current: "intent", visited: new Set<Stage>(["intent"]) }),
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
