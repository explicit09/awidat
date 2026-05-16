// Zustand store for the third-pane properties inspector. Holds a
// single nullable selection key — `{ trackIndex, clipIndex }` — that
// the TimelinePane writes on click and PropertiesPane reads to
// resolve which timeline item to show. The `clipIndex` name is kept
// for compatibility, but it is the selected TimelineItem.index and
// can identify clips or transitions.
//
// Stored as timeline item indices (not as the clip's uuid) so the
// canvas can use the same key for hit testing and selection paint.
// The properties pane resolves the key against the current snapshot.

import { create } from "zustand";

export type SelectedClipKey = {
  /** Index into TimelineSnapshot.tracks. */
  trackIndex: number;
  /** TimelineItem.index inside the chosen track. */
  clipIndex: number;
};

type State = {
  selectedClipKey: SelectedClipKey | null;
  select: (key: SelectedClipKey) => void;
  clear: () => void;
};

export const useTimelineSelectionStore = create<State>((set) => ({
  selectedClipKey: null,
  select: (key) => set({ selectedClipKey: key }),
  clear: () => set({ selectedClipKey: null }),
}));
