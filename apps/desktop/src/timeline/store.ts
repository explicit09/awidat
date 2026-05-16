// Zustand store for the timeline pane. Refreshes its snapshot on
// project change and whenever an apply_edl tool call lands in chat
// (the agent just edited the timeline; we want the canvas to reflect
// it).

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
// TimelineItem / TimelineTrack / TimelineSnapshot are generated from
// the Rust protocol crate via ts-rs — drift is impossible because
// `cargo test -p awidat-desktop-protocol` re-exports them on every
// build. The frontend re-exports them here so existing call sites
// don't have to learn a new import path.
export type {
  TimelineItem,
  TimelineTrack,
  TimelineSnapshot,
} from "../protocol";
import type { TimelineSnapshot } from "../protocol";

type State = {
  snapshot: TimelineSnapshot;
  /** True if the next refresh should auto-fit zoom. Cleared once consumed. */
  refreshing: boolean;
  zoom: number;
  refresh: () => Promise<void>;
  zoomIn: () => void;
  zoomOut: () => void;
  fitZoom: () => void;
};

export const useTimelineStore = create<State>((set) => ({
  snapshot: {
    duration_s: 0,
    broadcast_overlay: null,
    cut_boundaries: [],
    preview_limitations: [],
    tracks: [],
  },
  refreshing: false,
  zoom: 1,
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
  zoomIn: () =>
    set((state) => ({ zoom: Math.min(8, state.zoom * 1.25) })),
  zoomOut: () =>
    set((state) => ({ zoom: Math.max(0.25, state.zoom / 1.25) })),
  fitZoom: () => set({ zoom: 1 }),
}));
