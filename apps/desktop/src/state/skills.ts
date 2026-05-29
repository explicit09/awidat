/**
 * Per-project Skills store — UI state for the Skills surface.
 *
 * Tracks which skills the user has toggled OFF for each project root.
 * The default is ON: every discovered skill is enabled until the user
 * explicitly disables it. We store the disabled set (not enabled) so
 * newly added bundled skills don't silently arrive in a disabled
 * state when a project comes back after an upgrade.
 *
 * IMPORTANT — UI-only today:
 *   This toggle does NOT yet affect the agent's loadout. The
 *   `render_skills_catalog()` Tauri path that prepends the L1
 *   catalog to every turn doesn't read this store. Wiring the
 *   per-project disable into the backend is a follow-up task —
 *   we intentionally don't touch `codex_session.rs` from the UI
 *   tab.
 */

import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";

/**
 * Persisted shape: project path → array of disabled skill names.
 * Arrays + plain objects survive JSON round-tripping; the store
 * thaws them back into a `Map<string, Set<string>>` for ergonomic
 * access at read time.
 */
type PersistedShape = {
  /** project root path → disabled skill names */
  disabled: Record<string, string[]>;
};

export type SkillsStore = {
  /** Map of project root → set of skill names the user disabled. */
  disabledByProject: Map<string, Set<string>>;
  /** True when the skill is disabled for the given project root. */
  isDisabled: (projectRoot: string | null, skillName: string) => boolean;
  /** Toggle a skill's enabled state for the given project root. */
  toggle: (projectRoot: string | null, skillName: string) => void;
  /** Force a specific state. Useful for tests + future bulk actions. */
  setDisabled: (
    projectRoot: string | null,
    skillName: string,
    disabled: boolean,
  ) => void;
  /** Clear all disable state for a project (used by tests). */
  clearForProject: (projectRoot: string) => void;
};

/**
 * The localStorage key. Versioned so we can migrate the shape if
 * we ever switch from name-based to id-based skill keys.
 */
const STORAGE_KEY = "awidat:skills:disabled";

/**
 * No-op project key for when nothing's loaded. Centralizing this
 * here keeps callers from having to null-check on every read.
 */
const PROJECT_GLOBAL = "__global__";

function projectKey(projectRoot: string | null): string {
  return projectRoot ?? PROJECT_GLOBAL;
}

/**
 * Pure helpers — exported so tests can exercise the disable-state
 * logic without spinning up a Zustand store.
 */
export function computeIsDisabled(
  map: Map<string, Set<string>>,
  projectRoot: string | null,
  skillName: string,
): boolean {
  return map.get(projectKey(projectRoot))?.has(skillName) ?? false;
}

export function applyToggle(
  map: Map<string, Set<string>>,
  projectRoot: string | null,
  skillName: string,
): Map<string, Set<string>> {
  const next = new Map(map);
  const key = projectKey(projectRoot);
  const existing = new Set(next.get(key) ?? []);
  if (existing.has(skillName)) {
    existing.delete(skillName);
  } else {
    existing.add(skillName);
  }
  if (existing.size === 0) {
    next.delete(key);
  } else {
    next.set(key, existing);
  }
  return next;
}

export function applySetDisabled(
  map: Map<string, Set<string>>,
  projectRoot: string | null,
  skillName: string,
  disabled: boolean,
): Map<string, Set<string>> {
  const next = new Map(map);
  const key = projectKey(projectRoot);
  const existing = new Set(next.get(key) ?? []);
  if (disabled) {
    existing.add(skillName);
  } else {
    existing.delete(skillName);
  }
  if (existing.size === 0) {
    next.delete(key);
  } else {
    next.set(key, existing);
  }
  return next;
}

/**
 * Convert the in-memory Map<string, Set<string>> into the
 * JSON-friendly persisted shape (and back). Exported for tests.
 */
export function serialize(map: Map<string, Set<string>>): PersistedShape {
  const disabled: Record<string, string[]> = {};
  for (const [project, names] of map.entries()) {
    disabled[project] = Array.from(names).sort();
  }
  return { disabled };
}

export function deserialize(shape: PersistedShape | undefined): Map<string, Set<string>> {
  const map = new Map<string, Set<string>>();
  const source = shape?.disabled ?? {};
  for (const [project, names] of Object.entries(source)) {
    if (Array.isArray(names) && names.length > 0) {
      map.set(project, new Set(names));
    }
  }
  return map;
}

export const useSkillsStore = create<SkillsStore>()(
  persist(
    (set, get) => ({
      disabledByProject: new Map(),
      isDisabled: (projectRoot, skillName) =>
        computeIsDisabled(get().disabledByProject, projectRoot, skillName),
      toggle: (projectRoot, skillName) =>
        set((state) => ({
          disabledByProject: applyToggle(
            state.disabledByProject,
            projectRoot,
            skillName,
          ),
        })),
      setDisabled: (projectRoot, skillName, disabled) =>
        set((state) => ({
          disabledByProject: applySetDisabled(
            state.disabledByProject,
            projectRoot,
            skillName,
            disabled,
          ),
        })),
      clearForProject: (projectRoot) =>
        set((state) => {
          const next = new Map(state.disabledByProject);
          next.delete(projectKey(projectRoot));
          return { disabledByProject: next };
        }),
    }),
    {
      name: STORAGE_KEY,
      storage: createJSONStorage(() => localStorage),
      // Persist only the disabled map (serialized) — the action
      // functions get rebuilt by the store on rehydration.
      partialize: (state) => ({
        disabled: serialize(state.disabledByProject).disabled,
      }),
      // Rehydrate the persisted shape back into a Map of Sets.
      merge: (persisted, current) => {
        const persistedShape = persisted as PersistedShape | undefined;
        return {
          ...current,
          disabledByProject: deserialize(persistedShape),
        };
      },
    },
  ),
);
