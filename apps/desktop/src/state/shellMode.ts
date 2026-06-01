import { create } from "zustand";

/**
 * Which application shell to render:
 *   "stage"   — the 2026 cinematic Stage UX (default)
 *   "cockpit" — the legacy three-rail AppShell
 *
 * Persisted to localStorage so the choice survives reloads. Exposed in
 * both shells' chrome via <ShellModeToggle> so the user can flip live.
 */
export type ShellMode = "stage" | "cockpit";

const KEY = "awidat:shell-mode";

function load(): ShellMode {
  try {
    const v = typeof localStorage !== "undefined" ? localStorage.getItem(KEY) : null;
    return v === "cockpit" ? "cockpit" : "stage";
  } catch {
    return "stage";
  }
}
function persist(mode: ShellMode) {
  try {
    localStorage?.setItem(KEY, mode);
  } catch {
    /* ignore */
  }
}

type ShellModeStore = {
  mode: ShellMode;
  setMode: (mode: ShellMode) => void;
  toggle: () => void;
};

export const useShellMode = create<ShellModeStore>((set, get) => ({
  mode: load(),
  setMode: (mode) => {
    persist(mode);
    set({ mode });
  },
  toggle: () => {
    const next: ShellMode = get().mode === "stage" ? "cockpit" : "stage";
    persist(next);
    set({ mode: next });
  },
}));
