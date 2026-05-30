// useEpisodesStore — shared holder for the `get_project_episodes` snapshot.
//
// Background: App.tsx invokes `get_project_episodes` on boot, on focus,
// after every indexer run, and whenever the timeline-changed event
// fires. It kept the result in local React state, which worked for the
// (legacy) inline episodes card inside `IndexingDashboard` but left the
// Wave 6 evidence drill-downs without a way to subscribe.
//
// This store mirrors the `useIndexReadinessStore` pattern: tiny, owns
// just the snapshot + a setter. App.tsx is still the one calling the
// Tauri command; it just also writes the result here so the
// `evidenceAccessors` module (and the new EpisodesDrillDown) can read
// it without prop-drilling.
//
// The snapshot shape matches `IndexingEpisodeSummary` already produced
// by `projectEpisodesToIndexingSummary` in App.tsx — duplicating the
// type would create two sources of truth, so we re-export it from
// `shell/IndexingDashboard.ts` via a structural alias.

import { create } from "zustand";

/**
 * One episode row, mirrored from `IndexingEpisodeSummary.episodes[number]`
 * in `shell/IndexingDashboard.ts`. Re-declared here to keep this store
 * independent of the shell barrel (avoids the shell → state cycle).
 */
export interface EpisodesStoreEpisode {
  id: string;
  name: string;
  order: number;
  startS: number;
  endS: number;
  durationS: number;
  confidence: number;
  status: "accepted" | "review_needed" | "rejected";
  evidenceCount?: number;
}

export interface EpisodesSnapshot {
  total: number;
  accepted: number;
  reviewNeeded: number;
  rejected: number;
  episodes: EpisodesStoreEpisode[];
}

interface EpisodesState {
  /** Latest snapshot from `get_project_episodes`; undefined until the
   *  first load (or after project unload). */
  snapshot: EpisodesSnapshot | undefined;
  /** Replace the snapshot. Pass `undefined` on project unload / failure. */
  setSnapshot: (snapshot: EpisodesSnapshot | undefined) => void;
}

export const useEpisodesStore = create<EpisodesState>((set) => ({
  snapshot: undefined,
  setSnapshot: (snapshot) => set({ snapshot }),
}));
