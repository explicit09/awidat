// Zustand store for the third-pane properties inspector. Holds a
// single nullable selection key — `{ trackIndex, clipIndex }` — that
// the TimelinePane writes on click and PropertiesPane reads to
// resolve which clip's metadata to show.
//
// Stored as indices (not as the clip's uuid) because the timeline
// snapshot is the source of truth; resolving uuid → snapshot would
// require an O(N) scan on every paint. Indices are stable for the
// lifetime of one snapshot, and we clear the selection on every
// snapshot refresh in App.tsx so a structural change can't leave a
// dangling selection pointing at a now-deleted clip.

import { create } from "zustand";

export type SelectedClipKey = {
  /** Index into TimelineSnapshot.tracks. */
  trackIndex: number;
  /** Index into the chosen track's items array. */
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
