/**
 * useCenterModeStore — which surface is rendered in the Edit-tab
 * center pane (Wave 3 task B1).
 *
 * Three modes:
 *   - `brief`    : the agent's proposal stack (BriefSurface)
 *   - `source`   : the legacy PreviewSurface (Source Review)
 *   - `timeline` : an expanded TimelineHybrid that fills the center
 *
 * Persistence: per-project, in localStorage. The chosen mode for the
 * currently-open project survives a reload. Project switching restores
 * whatever was last picked for that project (defaulting per the rules
 * in `resolveDefaultMode`).
 *
 * Defaulting rules (only consulted when the project has no persisted
 * choice yet):
 *   - if `pendingCount > 0` OR `isFirstSessionEver(project)` → "brief"
 *   - otherwise                                              → "source"
 *
 * Why a separate store: the Brief surface, the chrome toggle, and the
 * App.tsx renderer all need the same flag. Co-locating it with `mode`
 * (pro/creator) would conflate two orthogonal concerns; co-locating
 * with the proposals store would entangle persistence with proposal
 * lifecycle. Tiny store, single responsibility.
 */

import { create } from "zustand";

export type CenterMode = "brief" | "source" | "timeline";

export const CENTER_MODES: readonly CenterMode[] = ["brief", "source", "timeline"];

export const CENTER_MODE_LABEL: Record<CenterMode, string> = {
  brief: "Brief",
  source: "Source",
  timeline: "Timeline",
};

export const STORAGE_KEY = "montage:center-mode";

interface StorageAdapter {
  getItem(k: string): string | null;
  setItem(k: string, v: string): void;
  removeItem(k: string): void;
}

const isValidMode = (v: unknown): v is CenterMode =>
  v === "brief" || v === "source" || v === "timeline";

function loadInitial(storage: StorageAdapter | null): Record<string, CenterMode> {
  if (!storage) return {};
  const raw = storage.getItem(STORAGE_KEY);
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: Record<string, CenterMode> = {};
    for (const [project, mode] of Object.entries(parsed as Record<string, unknown>)) {
      if (typeof project === "string" && isValidMode(mode)) {
        out[project] = mode;
      }
    }
    return out;
  } catch {
    return {};
  }
}

function persistMap(storage: StorageAdapter | null, map: Record<string, CenterMode>): void {
  if (!storage) return;
  try {
    storage.setItem(STORAGE_KEY, JSON.stringify(map));
  } catch {
    // Quota errors are non-fatal; worst case the user re-picks next session.
  }
}

interface CenterModeStore {
  /** Project path → chosen mode. Missing entries fall back to the default rules. */
  byProject: Record<string, CenterMode>;
  /** Read the active mode for `project`, applying the default-rule fallback. */
  get: (project: string | null, ctx?: DefaultCtx) => CenterMode;
  /** Explicit set — clicking a tab. Persists. */
  set: (project: string | null, mode: CenterMode) => void;
}

export interface DefaultCtx {
  /** Number of pending Brief proposals — when >0, default to brief. */
  pendingCount?: number;
  /** True when this project hasn't seen its intro turn yet. */
  isFirstSession?: boolean;
}

/**
 * Pure default resolution — exported for tests. The store consults this
 * when the project has no persisted choice yet. Keep this load-bearing:
 * it is the contract behind "Brief is the default surface when the
 * agent has work waiting".
 */
export function resolveDefaultMode(ctx: DefaultCtx | undefined): CenterMode {
  if (!ctx) return "source";
  if ((ctx.pendingCount ?? 0) > 0) return "brief";
  if (ctx.isFirstSession) return "brief";
  return "source";
}

interface CreateOpts {
  persist?: boolean;
  storage?: StorageAdapter;
}

/** Framework-agnostic factory so the store can be exercised under plain node. */
export function createCenterModeStore(opts: CreateOpts = {}) {
  const persist = opts.persist ?? true;
  const storage: StorageAdapter | null = persist
    ? opts.storage ?? (typeof localStorage !== "undefined" ? localStorage : null)
    : null;

  return create<CenterModeStore>((set, getState) => ({
    byProject: loadInitial(storage),
    get(project, ctx) {
      if (!project) return resolveDefaultMode(ctx);
      const entry = getState().byProject[project];
      if (entry) return entry;
      return resolveDefaultMode(ctx);
    },
    set(project, mode) {
      if (!project) return;
      set((state) => {
        if (state.byProject[project] === mode) return state;
        const next = { ...state.byProject, [project]: mode };
        persistMap(storage, next);
        return { byProject: next };
      });
    },
  }));
}

export const useCenterModeStore = createCenterModeStore();
