// Zustand store for the timeline pane. Refreshes its snapshot on
// project change and whenever an apply_edl tool call lands in chat
// (the agent just edited the timeline; we want the canvas to reflect
// it).

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";

export type TimelineItem =
  | {
      kind: "clip";
      index: number;
      name: string;
      track_start_s: number;
      duration_s: number;
      asset_id: string | null;
      source_start_s: number | null;
    }
  | {
      kind: "gap";
      index: number;
      track_start_s: number;
      duration_s: number;
    }
  | {
      kind: "transition";
      index: number;
      track_start_s: number;
      duration_s: number;
      effect_name: string;
    };

export type TimelineTrack = {
  name: string;
  kind: "video" | "audio" | string;
  items: TimelineItem[];
};

export type TimelineSnapshot = {
  duration_s: number;
  tracks: TimelineTrack[];
};

type State = {
  snapshot: TimelineSnapshot;
  /** True if the next refresh should auto-fit zoom. Cleared once consumed. */
  refreshing: boolean;
  refresh: () => Promise<void>;
};

export const useTimelineStore = create<State>((set) => ({
  snapshot: { duration_s: 0, tracks: [] },
  refreshing: false,
  refresh: async () => {
    set({ refreshing: true });
    try {
      const snapshot = await invoke<TimelineSnapshot>("read_timeline");
      set({ snapshot, refreshing: false });
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn("read_timeline failed", e);
      set({ refreshing: false });
    }
  },
}));
