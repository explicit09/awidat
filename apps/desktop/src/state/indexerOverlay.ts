/**
 * Per-project indexer overlay store — UI state for the IndexersStrip
 * enable/disable popover (Wave 4 T3).
 *
 * Tracks which indexers the user has toggled OFF for each project root.
 * The default is ON: every server discovered via the merged config is
 * active until the user explicitly disables it. We store the disabled
 * set (not enabled) so newly added indexers don't silently arrive in a
 * disabled state when a project comes back after an upgrade.
 *
 * Backend integration:
 *   - On every mutation (toggle / setDisabled) the store invokes the
 *     `write_disabled_indexers` Tauri command for the affected project,
 *     persisting the list to `<project>/.montage/indexers.json`. The
 *     dispatcher (`index_project_at_root`) reads the same file at run
 *     time and drops any disabled server before launching.
 *   - `hydrateFromDisk(projectRoot)` lets the app glue prime the store
 *     from `.montage/indexers.json` whenever a project opens, mirroring
 *     state that landed via file sync (Dropbox/git/etc.).
 *
 * Mirrors `state/skills.ts` so the W4.1 pattern stays the only one a
 * future contributor has to understand to add a third overlay.
 */

import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";
import { invoke, isTauri } from "@tauri-apps/api/core";

/**
 * Persisted shape: project path → array of disabled indexer names.
 * Plain object survives JSON round-tripping; the store thaws it back
 * into a `Map<string, Set<string>>` for ergonomic access at read time.
 */
type PersistedShape = {
  /** project root path → disabled indexer names */
  disabled: Record<string, string[]>;
};

export type IndexerOverlayStore = {
  /** Map of project root → set of indexer names the user disabled. */
  disabledByProject: Map<string, Set<string>>;
  /** True when the indexer is disabled for the given project root. */
  isDisabled: (projectRoot: string | null, indexerName: string) => boolean;
  /** Names of disabled indexers for a project root (sorted, plain array). */
  disabledFor: (projectRoot: string | null) => string[];
  /** Toggle an indexer's enabled state for the given project root. */
  toggle: (projectRoot: string | null, indexerName: string) => void;
  /** Force a specific state. Useful for tests + future bulk actions. */
  setDisabled: (
    projectRoot: string | null,
    indexerName: string,
    disabled: boolean,
  ) => void;
  /**
   * Replace the in-memory disabled set for a project with the names
   * read from `.montage/indexers.json` via the backend. Does NOT persist
   * back to disk — only mirrors what the file already contains. Called
   * by the app glue on project open so the toggle UI reflects any
   * state that arrived via file sync.
   */
  hydrateFromDisk: (projectRoot: string, disabled: string[]) => void;
  /** Clear all disable state for a project (used by tests). */
  clearForProject: (projectRoot: string) => void;
};

/**
 * The localStorage key. Versioned so we can migrate the shape if we
 * ever switch from name-based to id-based indexer keys.
 */
const STORAGE_KEY = "montage:indexers:disabled";

/**
 * No-op project key for when nothing's loaded. Centralizing this here
 * keeps callers from having to null-check on every read.
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
  indexerName: string,
): boolean {
  return map.get(projectKey(projectRoot))?.has(indexerName) ?? false;
}

export function applyToggle(
  map: Map<string, Set<string>>,
  projectRoot: string | null,
  indexerName: string,
): Map<string, Set<string>> {
  const next = new Map(map);
  const key = projectKey(projectRoot);
  const existing = new Set(next.get(key) ?? []);
  if (existing.has(indexerName)) {
    existing.delete(indexerName);
  } else {
    existing.add(indexerName);
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
  indexerName: string,
  disabled: boolean,
): Map<string, Set<string>> {
  const next = new Map(map);
  const key = projectKey(projectRoot);
  const existing = new Set(next.get(key) ?? []);
  if (disabled) {
    existing.add(indexerName);
  } else {
    existing.delete(indexerName);
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
 * Convert the in-memory Map<string, Set<string>> into the JSON-friendly
 * persisted shape (and back). Exported for tests.
 */
export function serialize(map: Map<string, Set<string>>): PersistedShape {
  const disabled: Record<string, string[]> = {};
  for (const [project, names] of map.entries()) {
    disabled[project] = Array.from(names).sort();
  }
  return { disabled };
}

export function deserialize(
  shape: PersistedShape | undefined,
): Map<string, Set<string>> {
  const map = new Map<string, Set<string>>();
  const source = shape?.disabled ?? {};
  for (const [project, names] of Object.entries(source)) {
    if (Array.isArray(names) && names.length > 0) {
      map.set(project, new Set(names));
    }
  }
  return map;
}

/**
 * Persistence hook — invoked after every mutation that affects a real
 * project root. Pulled out as a swappable function so tests can spy on
 * it without standing up Tauri's IPC bridge.
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
      await invoke("write_disabled_indexers", {
        projectPath: projectRoot,
        disabled,
      });
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("write_disabled_indexers failed", err);
    }
  };

/**
 * Test seam — override the persistence sink. Returns the previous
 * implementation so callers can restore it in their teardown.
 */
export function __setPersistDisabledForTests(
  next: (projectRoot: string, disabled: string[]) => Promise<void>,
): (projectRoot: string, disabled: string[]) => Promise<void> {
  const prev = persistDisabled;
  persistDisabled = next;
  return prev;
}

/** Read the disabled set for a project as a plain sorted array. */
function disabledForMap(
  map: Map<string, Set<string>>,
  projectRoot: string | null,
): string[] {
  const set = map.get(projectKey(projectRoot));
  return set ? Array.from(set).sort() : [];
}

export const useIndexerOverlay = create<IndexerOverlayStore>()(
  persist(
    (set, get) => ({
      disabledByProject: new Map(),
      isDisabled: (projectRoot, indexerName) =>
        computeIsDisabled(get().disabledByProject, projectRoot, indexerName),
      disabledFor: (projectRoot) =>
        disabledForMap(get().disabledByProject, projectRoot),
      toggle: (projectRoot, indexerName) => {
        const nextMap = applyToggle(get().disabledByProject, projectRoot, indexerName);
        set({ disabledByProject: nextMap });
        // Only persist when we have a real project root — the global
        // bucket has no on-disk counterpart by design.
        if (projectRoot !== null) {
          void persistDisabled(projectRoot, disabledForMap(nextMap, projectRoot));
        }
      },
      setDisabled: (projectRoot, indexerName, disabled) => {
        const nextMap = applySetDisabled(
          get().disabledByProject,
          projectRoot,
          indexerName,
          disabled,
        );
        set({ disabledByProject: nextMap });
        if (projectRoot !== null) {
          void persistDisabled(projectRoot, disabledForMap(nextMap, projectRoot));
        }
      },
      hydrateFromDisk: (projectRoot, disabled) =>
        set((state) => ({
          disabledByProject: applyHydrate(
            state.disabledByProject,
            projectRoot,
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
