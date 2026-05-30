// App-shell state. Project lifecycle lives outside the agent store
// because the project root predates / outlives any single Session.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { ProjectType } from "../protocol";

type ProjectState = {
  /** Absolute path to the current project, or null if none loaded. */
  current: string | null;
  /** Most recently-opened paths, newest first. */
  recent: string[];
  /**
   * Active project's declared shape (podcast | shorts | tutorial |
   * other). `null` when no project is loaded or the fetch hasn't
   * completed. Subscribed by the Source-view router so podcast /
   * interview / tutorial projects land on the transcript-first
   * surface while other types stay on the video preview.
   */
  projectType: ProjectType | null;
  /** Refresh both from the backend. */
  refresh: () => Promise<void>;
  /** Locally update `current` without round-tripping the backend. */
  setCurrent: (path: string | null) => void;
  /** Re-fetch only the project type — called when `current` changes. */
  refreshProjectType: () => Promise<void>;
};

export const useProjectStore = create<ProjectState>((set, get) => ({
  current: null,
  recent: [],
  projectType: null,
  refresh: async () => {
    const [current, recent] = await Promise.all([
      invoke<string | null>("current_project_root"),
      invoke<string[]>("recent_projects"),
    ]);
    set({ current, recent });
    await get().refreshProjectType();
  },
  setCurrent: (current) => {
    set({ current, projectType: current ? get().projectType : null });
    if (current) void get().refreshProjectType();
  },
  refreshProjectType: async () => {
    if (!get().current) {
      set({ projectType: null });
      return;
    }
    try {
      const pt = await invoke<ProjectType>("get_project_type");
      set({ projectType: pt });
    } catch {
      set({ projectType: null });
    }
  },
}));
