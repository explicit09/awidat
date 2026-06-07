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
 * surface union; `schedule`, `skills`, and `history` are non-linear
 * tabs that live next to the workflow stages in the WorkspaceRow but
 * don't participate in the progress indicator.
 */
export type Stage = (typeof STAGES)[number] | "schedule" | "skills" | "history";

/**
 * Routable product surfaces shown in top-level workspace chrome.
 * `STAGES` remains the linear edit -> deliver workflow; this list is the
 * places a user can jump to directly from the product surface.
 */
export const WORKSPACE_DESTINATIONS = [
  "edit",
  "deliver",
  "schedule",
  "skills",
  "history",
] as const satisfies readonly Stage[];

export const STAGE_LABEL: Record<Stage, string> = {
  edit: "Edit",
  deliver: "Deliver",
  schedule: "Schedule",
  skills: "Skills",
  history: "History",
};

export type WorkspaceShortcut = {
  stage: Stage;
  keys: string;
  label: string;
};

export const WORKSPACE_SHORTCUTS = WORKSPACE_DESTINATIONS.map((stage, index) => ({
  stage,
  keys: `⌘${index + 1}`,
  label: STAGE_LABEL[stage],
})) as readonly WorkspaceShortcut[];

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

/** Dev-only: boot straight into a stage via `VITE_MONTAGE_STAGE=deliver`
 *  (used for native screenshot tours; ignored in production builds). */
const DEV_INITIAL_STAGE = ((): Stage => {
  const v = import.meta.env?.VITE_MONTAGE_STAGE as string | undefined;
  return v === "deliver" || v === "schedule" || v === "skills" || v === "history" ? v : "edit";
})();

export const useStageStore = create<StageStore>((set) => ({
  current: DEV_INITIAL_STAGE,
  visited: new Set<Stage>(["edit", DEV_INITIAL_STAGE]),
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

export function stageFromWorkspaceShortcut(
  key: string,
  modifierPressed: boolean,
): Stage | null {
  if (!modifierPressed) return null;
  const index = Number(key) - 1;
  if (!Number.isInteger(index) || index < 0 || index >= WORKSPACE_DESTINATIONS.length) {
    return null;
  }
  return WORKSPACE_DESTINATIONS[index] ?? null;
}
