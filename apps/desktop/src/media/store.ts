// Zustand store for the media pane: which assets have proxies on
// disk, which one's currently selected for playback, current
// playback time. Refreshed when transcode jobs land.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";

export type ProxyEntry = {
  stem: string;
  proxy_path: string;
  size_bytes: number;
};

type MediaState = {
  /** All proxies in `.awidat/proxies/`, sorted by stem. */
  proxies: ProxyEntry[];
  /** Stem of the currently-selected asset, or null if none / no proxies. */
  selectedStem: string | null;
  /** Refresh from `list_proxies`. */
  refresh: () => Promise<void>;
  /** Pick which asset plays. */
  select: (stem: string | null) => void;
};

export const useMediaStore = create<MediaState>((set, get) => ({
  proxies: [],
  selectedStem: null,
  refresh: async () => {
    try {
      const proxies = await invoke<ProxyEntry[]>("list_proxies");
      set((state) => {
        // Keep current selection if still present; otherwise pick
        // the first proxy if any. Falling back to null when the
        // project has no proxies yet.
        const stillThere =
          state.selectedStem !== null &&
          proxies.some((p) => p.stem === state.selectedStem);
        const selectedStem = stillThere
          ? state.selectedStem
          : proxies[0]?.stem ?? null;
        return { proxies, selectedStem };
      });
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn("list_proxies failed", e);
    }
  },
  select: (stem) => {
    if (stem === null) {
      set({ selectedStem: null });
      return;
    }
    const proxies = get().proxies;
    if (proxies.some((p) => p.stem === stem)) {
      set({ selectedStem: stem });
    }
  },
}));
