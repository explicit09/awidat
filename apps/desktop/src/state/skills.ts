/**
 * Per-project Skills store — UI state for the Skills surface.
 *
 * Tracks two pieces of project-scoped state:
 *   1. **Disabled skills** — skills the editor has explicitly toggled
 *      OFF. Default is ON: every discovered skill is enabled until
 *      the user disables it. We store the disabled set (not enabled)
 *      so newly added bundled skills don't silently arrive disabled
 *      after an upgrade.
 *   2. **Pinned skills** (Wave 5 B2) — version + provenance locks.
 *      A pinned skill must match the pinned `version` and/or come
 *      from the pinned `provenance` layer; otherwise it's skipped at
 *      catalog assembly time. Disabled wins over pinned (a disabled
 *      skill stays disabled).
 *
 * Backend integration (Wave 4 T1 + Wave 5 B2):
 *   - Disabled mutations call `write_disabled_skills`, which preserves
 *     any existing pins on disk.
 *   - Pin mutations call `write_skill_config` with the full shape so
 *     pins land alongside the disabled list.
 *   - `hydrateFromDisk(projectRoot, disabled, pinned)` primes the
 *     store from `.awidat/skills.json` on project open, reflecting
 *     state that landed via file sync (Dropbox/git/etc.).
 *
 * The localStorage cache survives across reloads and across sessions
 * where the file hasn't been hydrated yet — it's a UX optimization,
 * not the source of truth. The on-disk file is canonical.
 */

import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";
import { invoke, isTauri } from "@tauri-apps/api/core";

/**
 * One pin entry — locks a skill to a specific version and/or
 * provenance layer. At least one of `version` / `provenance` should
 * be set for the pin to do anything; a pin with neither is a no-op
 * (kept rather than rejected so manual edits / file sync don't fail
 * the loader).
 */
export type PinnedSkill = {
  /** Skill name (matches `SkillMeta::name`). */
  name: string;
  /** Required exact version (`MAJOR.MINOR.PATCH`). Omitted = any. */
  version?: string;
  /** Required discovery layer. Omitted = any. */
  provenance?: "bundled" | "user" | "project";
};

/**
 * Persisted shape: project path → disabled names + pin list.
 * Arrays + plain objects survive JSON round-tripping; the store
 * thaws them back into Maps + Sets for ergonomic access at read time.
 */
type PersistedShape = {
  /** project root path → disabled skill names */
  disabled: Record<string, string[]>;
  /** project root path → pinned skill list */
  pinned?: Record<string, PinnedSkill[]>;
};

export type SkillsStore = {
  /** Map of project root → set of skill names the user disabled. */
  disabledByProject: Map<string, Set<string>>;
  /** Map of project root → pin list (per skill name). */
  pinnedByProject: Map<string, PinnedSkill[]>;
  /** True when the skill is disabled for the given project root. */
  isDisabled: (projectRoot: string | null, skillName: string) => boolean;
  /** Return the pin for a skill, or `undefined` when none. */
  getPin: (
    projectRoot: string | null,
    skillName: string,
  ) => PinnedSkill | undefined;
  /** Toggle a skill's enabled state for the given project root. */
  toggle: (projectRoot: string | null, skillName: string) => void;
  /** Force a specific state. Useful for tests + future bulk actions. */
  setDisabled: (
    projectRoot: string | null,
    skillName: string,
    disabled: boolean,
  ) => void;
  /** Add (or replace) a pin for `skillName`. */
  setPin: (
    projectRoot: string | null,
    pin: PinnedSkill,
  ) => void;
  /** Drop the pin for `skillName` if any. */
  clearPin: (projectRoot: string | null, skillName: string) => void;
  /**
   * Replace the in-memory disabled set + pin list for a project with
   * the values read from `.awidat/skills.json` via the backend. Does
   * NOT persist back to disk — only mirrors what the file already
   * contains. Called by the app glue on project open so the toggle
   * UI reflects any state that arrived via file sync.
   */
  hydrateFromDisk: (
    projectRoot: string,
    disabled: string[],
    pinned: PinnedSkill[],
  ) => void;
  /** Clear all per-project state (used by tests). */
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
 * Replace the project's disabled set wholesale. Used by the
 * hydrate-from-disk path — the on-disk file is authoritative for the
 * project's enabled/disabled state, so we don't merge with any prior
 * UI state; we replace it.
 */
export function applyHydrate(
  map: Map<string, Set<string>>,
  projectRoot: string,
  disabled: string[],
): Map<string, Set<string>> {
  const next = new Map(map);
  const key = projectKey(projectRoot);
  if (disabled.length === 0) {
    next.delete(key);
  } else {
    next.set(key, new Set(disabled));
  }
  return next;
}

/**
 * Pin helpers. Pins are stored as a sorted array per project (sorted
 * by name on write so on-disk diffs stay stable). Last-write-wins
 * within a project — `applySetPin` replaces any existing pin for the
 * same skill name.
 */
export function computeGetPin(
  map: Map<string, PinnedSkill[]>,
  projectRoot: string | null,
  skillName: string,
): PinnedSkill | undefined {
  return map
    .get(projectKey(projectRoot))
    ?.find((p) => p.name === skillName);
}

export function applySetPin(
  map: Map<string, PinnedSkill[]>,
  projectRoot: string | null,
  pin: PinnedSkill,
): Map<string, PinnedSkill[]> {
  const next = new Map(map);
  const key = projectKey(projectRoot);
  const existing = next.get(key) ?? [];
  // Remove any prior pin for the same skill name then insert the new
  // one. Keeps the list deduped by name (the backend dedupes too,
  // belt-and-suspenders).
  const filtered = existing.filter((p) => p.name !== pin.name);
  filtered.push(pin);
  filtered.sort((a, b) => a.name.localeCompare(b.name));
  next.set(key, filtered);
  return next;
}

export function applyClearPin(
  map: Map<string, PinnedSkill[]>,
  projectRoot: string | null,
  skillName: string,
): Map<string, PinnedSkill[]> {
  const next = new Map(map);
  const key = projectKey(projectRoot);
  const existing = next.get(key);
  if (!existing) return next;
  const filtered = existing.filter((p) => p.name !== skillName);
  if (filtered.length === 0) {
    next.delete(key);
  } else {
    next.set(key, filtered);
  }
  return next;
}

export function applyHydratePins(
  map: Map<string, PinnedSkill[]>,
  projectRoot: string,
  pinned: PinnedSkill[],
): Map<string, PinnedSkill[]> {
  const next = new Map(map);
  const key = projectKey(projectRoot);
  if (pinned.length === 0) {
    next.delete(key);
  } else {
    // Defensive copy + sort to match write-side ordering.
    const copy = pinned.map((p) => ({ ...p }));
    copy.sort((a, b) => a.name.localeCompare(b.name));
    next.set(key, copy);
  }
  return next;
}

/**
 * Convert the in-memory Map<string, Set<string>> + Map<string, PinnedSkill[]>
 * into the JSON-friendly persisted shape (and back). Exported for tests.
 */
export function serialize(
  disabledMap: Map<string, Set<string>>,
  pinnedMap: Map<string, PinnedSkill[]> = new Map(),
): PersistedShape {
  const disabled: Record<string, string[]> = {};
  for (const [project, names] of disabledMap.entries()) {
    disabled[project] = Array.from(names).sort();
  }
  const pinned: Record<string, PinnedSkill[]> = {};
  for (const [project, list] of pinnedMap.entries()) {
    if (list.length === 0) continue;
    pinned[project] = list.map((p) => ({ ...p }));
  }
  return { disabled, pinned };
}

export function deserialize(shape: PersistedShape | undefined): {
  disabled: Map<string, Set<string>>;
  pinned: Map<string, PinnedSkill[]>;
} {
  const disabled = new Map<string, Set<string>>();
  const source = shape?.disabled ?? {};
  for (const [project, names] of Object.entries(source)) {
    if (Array.isArray(names) && names.length > 0) {
      disabled.set(project, new Set(names));
    }
  }
  const pinned = new Map<string, PinnedSkill[]>();
  const pinSource = shape?.pinned ?? {};
  for (const [project, list] of Object.entries(pinSource)) {
    if (Array.isArray(list) && list.length > 0) {
      pinned.set(
        project,
        list.map((p) => ({ ...p })),
      );
    }
  }
  return { disabled, pinned };
}

/**
 * Persistence hooks — invoked after every mutation that affects a
 * real project root. Pulled out as swappable functions so tests can
 * spy on them without standing up Tauri's IPC bridge.
 *
 * When `projectRoot` is `null` the toggle is operating on the global
 * bucket (no project loaded) and we have nowhere to write to, so we
 * skip — the localStorage cache still keeps the UI consistent.
 *
 * Errors are logged but swallowed: the on-screen toggle has already
 * applied, and surfacing a Rust IPC failure to the user mid-toggle
 * would be noisier than the actual problem. The next mutation retries
 * the write.
 */
let persistDisabled: (projectRoot: string, disabled: string[]) => Promise<void> =
  async (projectRoot, disabled) => {
    if (!isTauri()) return;
    try {
      await invoke("write_disabled_skills", {
        projectPath: projectRoot,
        disabled,
      });
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("write_disabled_skills failed", err);
    }
  };

let persistSkillConfig: (
  projectRoot: string,
  disabled: string[],
  pinned: PinnedSkill[],
) => Promise<void> = async (projectRoot, disabled, pinned) => {
  if (!isTauri()) return;
  try {
    await invoke("write_skill_config", {
      projectPath: projectRoot,
      config: { version: 2, disabled, pinned },
    });
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn("write_skill_config failed", err);
  }
};

/**
 * Test seams — override the persistence sinks. Returns the previous
 * implementation so callers can restore it in their teardown.
 */
export function __setPersistDisabledForTests(
  next: (projectRoot: string, disabled: string[]) => Promise<void>,
): (projectRoot: string, disabled: string[]) => Promise<void> {
  const prev = persistDisabled;
  persistDisabled = next;
  return prev;
}

export function __setPersistSkillConfigForTests(
  next: (
    projectRoot: string,
    disabled: string[],
    pinned: PinnedSkill[],
  ) => Promise<void>,
): (
  projectRoot: string,
  disabled: string[],
  pinned: PinnedSkill[],
) => Promise<void> {
  const prev = persistSkillConfig;
  persistSkillConfig = next;
  return prev;
}

/** Read the disabled set for a project as a plain sorted array. */
function disabledFor(
  map: Map<string, Set<string>>,
  projectRoot: string,
): string[] {
  const set = map.get(projectKey(projectRoot));
  return set ? Array.from(set).sort() : [];
}

/** Read the pin list for a project as a plain sorted array. */
function pinnedFor(
  map: Map<string, PinnedSkill[]>,
  projectRoot: string,
): PinnedSkill[] {
  const list = map.get(projectKey(projectRoot));
  return list ? list.map((p) => ({ ...p })) : [];
}

export const useSkillsStore = create<SkillsStore>()(
  persist(
    (set, get) => ({
      disabledByProject: new Map(),
      pinnedByProject: new Map(),
      isDisabled: (projectRoot, skillName) =>
        computeIsDisabled(get().disabledByProject, projectRoot, skillName),
      getPin: (projectRoot, skillName) =>
        computeGetPin(get().pinnedByProject, projectRoot, skillName),
      toggle: (projectRoot, skillName) => {
        const nextMap = applyToggle(get().disabledByProject, projectRoot, skillName);
        set({ disabledByProject: nextMap });
        // Only persist when we have a real project root — the global
        // bucket has no on-disk counterpart by design.
        if (projectRoot !== null) {
          void persistDisabled(projectRoot, disabledFor(nextMap, projectRoot));
        }
      },
      setDisabled: (projectRoot, skillName, disabled) => {
        const nextMap = applySetDisabled(
          get().disabledByProject,
          projectRoot,
          skillName,
          disabled,
        );
        set({ disabledByProject: nextMap });
        if (projectRoot !== null) {
          void persistDisabled(projectRoot, disabledFor(nextMap, projectRoot));
        }
      },
      setPin: (projectRoot, pin) => {
        const nextPins = applySetPin(get().pinnedByProject, projectRoot, pin);
        set({ pinnedByProject: nextPins });
        if (projectRoot !== null) {
          // Pins go through the full-config writer so we preserve
          // disabled state on disk in the same write.
          void persistSkillConfig(
            projectRoot,
            disabledFor(get().disabledByProject, projectRoot),
            pinnedFor(nextPins, projectRoot),
          );
        }
      },
      clearPin: (projectRoot, skillName) => {
        const nextPins = applyClearPin(
          get().pinnedByProject,
          projectRoot,
          skillName,
        );
        set({ pinnedByProject: nextPins });
        if (projectRoot !== null) {
          void persistSkillConfig(
            projectRoot,
            disabledFor(get().disabledByProject, projectRoot),
            pinnedFor(nextPins, projectRoot),
          );
        }
      },
      hydrateFromDisk: (projectRoot, disabled, pinned) =>
        set((state) => ({
          disabledByProject: applyHydrate(
            state.disabledByProject,
            projectRoot,
            disabled,
          ),
          pinnedByProject: applyHydratePins(
            state.pinnedByProject,
            projectRoot,
            pinned,
          ),
        })),
      clearForProject: (projectRoot) =>
        set((state) => {
          const nextDisabled = new Map(state.disabledByProject);
          nextDisabled.delete(projectKey(projectRoot));
          const nextPinned = new Map(state.pinnedByProject);
          nextPinned.delete(projectKey(projectRoot));
          return {
            disabledByProject: nextDisabled,
            pinnedByProject: nextPinned,
          };
        }),
    }),
    {
      name: STORAGE_KEY,
      storage: createJSONStorage(() => localStorage),
      // Persist only the disabled + pinned maps (serialized) — the
      // action functions get rebuilt by the store on rehydration.
      partialize: (state) => {
        const shape = serialize(state.disabledByProject, state.pinnedByProject);
        return { disabled: shape.disabled, pinned: shape.pinned };
      },
      // Rehydrate the persisted shape back into Maps.
      merge: (persisted, current) => {
        const persistedShape = persisted as PersistedShape | undefined;
        const restored = deserialize(persistedShape);
        return {
          ...current,
          disabledByProject: restored.disabled,
          pinnedByProject: restored.pinned,
        };
      },
    },
  ),
);
