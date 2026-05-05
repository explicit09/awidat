// Zustand store for the media pane: which assets have proxies on
// disk, which one's currently selected for playback, and live
// playback state (current time, duration, isPlaying). The next
// commit will wire the playback state into the agent's per-turn
// context so the agent knows what the user is looking at.

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
  /** Live playhead position in seconds. */
  currentTime: number;
  /** Source duration in seconds (0 until the video element loads metadata). */
  durationS: number;
  /** True between play() and pause()/ended. */
  isPlaying: boolean;

  /** Refresh from `list_proxies`. */
  refresh: () => Promise<void>;
  /** Pick which asset plays. Resets playback state. */
  select: (stem: string | null) => void;
  /** Called by the player on `timeupdate` / scrub. */
  setTime: (t: number) => void;
  /** Called on `loadedmetadata`. */
  setDuration: (d: number) => void;
  /** Called on `play` / `pause` / `ended`. */
  setPlaying: (p: boolean) => void;
};

export const useMediaStore = create<MediaState>((set, get) => ({
  proxies: [],
  selectedStem: null,
  currentTime: 0,
  durationS: 0,
  isPlaying: false,

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
      set({ selectedStem: null, currentTime: 0, durationS: 0, isPlaying: false });
      return;
    }
    const proxies = get().proxies;
    if (proxies.some((p) => p.stem === stem)) {
      set({
        selectedStem: stem,
        currentTime: 0,
        durationS: 0,
        isPlaying: false,
      });
    }
  },
  setTime: (t) => set({ currentTime: t }),
  setDuration: (d) => set({ durationS: d }),
  setPlaying: (p) => set({ isPlaying: p }),
}));
