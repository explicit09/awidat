// App-shell state. Project lifecycle lives outside the agent store
// because the project root predates / outlives any single Session.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";

type ProjectState = {
  /** Absolute path to the current project, or null if none loaded. */
  current: string | null;
  /** Most recently-opened paths, newest first. */
  recent: string[];
  /** Refresh both from the backend. */
  refresh: () => Promise<void>;
  /** Locally update `current` without round-tripping the backend. */
  setCurrent: (path: string | null) => void;
};

export const useProjectStore = create<ProjectState>((set) => ({
  current: null,
  recent: [],
  refresh: async () => {
    const [current, recent] = await Promise.all([
      invoke<string | null>("current_project_root"),
      invoke<string[]>("recent_projects"),
    ]);
    set({ current, recent });
  },
  setCurrent: (current) => set({ current }),
}));
