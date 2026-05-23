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
  /** Horizontal zoom multiplier (1.0 = fit-to-viewport). */
  zoom: number;
  /** Vertical zoom multiplier — scales LANE_HEIGHT so tracks can be
   *  made taller for filmstrip-rich timelines. Independent from
   *  horizontal zoom (DaVinci/Premiere model). */
  trackZoom: number;
  refresh: () => Promise<void>;
  zoomIn: () => void;
  zoomOut: () => void;
  fitZoom: () => void;
  setZoom: (zoom: number) => void;
  setTrackZoom: (zoom: number) => void;
  trackZoomIn: () => void;
  trackZoomOut: () => void;
  fitTrackZoom: () => void;
};

const ZOOM_MIN = 0.25;
const ZOOM_MAX = 8;
const TRACK_ZOOM_MIN = 0.6;
const TRACK_ZOOM_MAX = 3;
const ZOOM_STEP = 1.25;
const TRACK_ZOOM_STEP = 1.15;

const PERSIST_KEY = "awidat.timeline.zoom.v1";

type PersistedShape = {
  zoom?: number;
  trackZoom?: number;
};

function loadPersisted(): PersistedShape {
  if (typeof localStorage === "undefined") return {};
  try {
    const raw = localStorage.getItem(PERSIST_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as PersistedShape;
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

function persist(state: Pick<State, "zoom" | "trackZoom">) {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(
      PERSIST_KEY,
      JSON.stringify({ zoom: state.zoom, trackZoom: state.trackZoom }),
    );
  } catch {
    // localStorage may be disabled (private mode); silently ignore.
  }
}

function clamp(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.min(max, Math.max(min, value));
}

const persisted = loadPersisted();

export const useTimelineStore = create<State>((set, get) => ({
  snapshot: {
    duration_s: 0,
    broadcast_overlay: null,
    cut_boundaries: [],
    preview_limitations: [],
    tracks: [],
  },
  refreshing: false,
  zoom: clamp(persisted.zoom ?? 1, ZOOM_MIN, ZOOM_MAX),
  trackZoom: clamp(persisted.trackZoom ?? 1, TRACK_ZOOM_MIN, TRACK_ZOOM_MAX),
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
  zoomIn: () => {
    const next = clamp(get().zoom * ZOOM_STEP, ZOOM_MIN, ZOOM_MAX);
    set({ zoom: next });
    persist(get());
  },
  zoomOut: () => {
    const next = clamp(get().zoom / ZOOM_STEP, ZOOM_MIN, ZOOM_MAX);
    set({ zoom: next });
    persist(get());
  },
  fitZoom: () => {
    set({ zoom: 1 });
    persist(get());
  },
  setZoom: (zoom) => {
    set({ zoom: clamp(zoom, ZOOM_MIN, ZOOM_MAX) });
    persist(get());
  },
  setTrackZoom: (trackZoom) => {
    set({ trackZoom: clamp(trackZoom, TRACK_ZOOM_MIN, TRACK_ZOOM_MAX) });
    persist(get());
  },
  trackZoomIn: () => {
    const next = clamp(get().trackZoom * TRACK_ZOOM_STEP, TRACK_ZOOM_MIN, TRACK_ZOOM_MAX);
    set({ trackZoom: next });
    persist(get());
  },
  trackZoomOut: () => {
    const next = clamp(get().trackZoom / TRACK_ZOOM_STEP, TRACK_ZOOM_MIN, TRACK_ZOOM_MAX);
    set({ trackZoom: next });
    persist(get());
  },
  fitTrackZoom: () => {
    set({ trackZoom: 1 });
    persist(get());
  },
}));

export const TIMELINE_ZOOM_BOUNDS = {
  min: ZOOM_MIN,
  max: ZOOM_MAX,
  trackMin: TRACK_ZOOM_MIN,
  trackMax: TRACK_ZOOM_MAX,
} as const;
