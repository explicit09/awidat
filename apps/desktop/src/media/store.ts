// Zustand store for the media pane.
//
// There are TWO clocks here:
//
// 1. **Source-time** (`currentTime`, `durationS`, `seek*`): seconds
//    *into a single proxy mp4*. Used internally by the video element
//    when it's playing the raw asset (empty-timeline / source-preview
//    mode) or one segment of the timeline at a time.
//
// 2. **Timeline-time** (`timelineTime`, `timelineDurationS`,
//    `timelineSeek*`): seconds along the master OTIO timeline.
//    Spans cuts between clips. This is the clock the user thinks in
//    when they look at the scrub bar — they care about "what plays
//    when I let the playhead run", not "what offset into raw
//    foo.MOV." The SegmentedVideoView (Step 9.3) owns the
//    translation between the two clocks.
//
// External callers (timeline canvas click, transcript click) use
// `requestTimelineSeek` once Step 9.3 lands. Until then,
// `requestSeek` (source-time) remains the only path so the existing
// raw-asset preview keeps working.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";

export type ProxyEntry = {
  stem: string;
  proxy_path: string;
  size_bytes: number;
};

export type SourceMediaEntry = {
  id: string;
  name: string;
  path: string;
  size_bytes: number;
};

type MediaState = {
  /** All source assets under `raw/`, sorted by project-relative id. */
  sources: SourceMediaEntry[];
  /** All proxies in `.montage/proxies/`, sorted by stem. */
  proxies: ProxyEntry[];
  /** Stem of the currently-selected asset, or null if none / no proxies. */
  selectedStem: string | null;
  /** Live playhead position in source-time seconds. */
  currentTime: number;
  /** Source duration in seconds (0 until the video element loads metadata). */
  durationS: number;
  /** True between play() and pause()/ended. */
  isPlaying: boolean;
  /** Last media playback/load failure surfaced by a video element. */
  mediaError: string | null;
  /**
   * Monotonically-increasing tick that bumps when an external caller
   * requests a source-time seek (e.g. the timeline canvas in
   * source-preview mode). The MediaPane subscribes to this and
   * imperatively sets `videoRef.currentTime` to `seekTargetS`. We
   * can't drive seeks via `currentTime` alone because the player
   * itself owns currentTime — the store mirrors it, doesn't command
   * it. Bumping a separate counter lets the player distinguish
   * "store updated because I emitted timeupdate" from "an external
   * caller wants me to seek now."
   */
  seekRequestId: number;
  /** Target seconds for the most recent source-time seek request. */
  seekTargetS: number;

  /** Live playhead position in timeline-time seconds. */
  timelineTime: number;
  /**
   * Total timeline duration in seconds. Mirrors
   * `useTimelineStore().snapshot.duration_s`; updated by the
   * MediaPane on snapshot change so the scrub bar can clamp
   * without subscribing to the timeline store directly.
   */
  timelineDurationS: number;
  /** Monotonic tick for timeline-time seek requests. */
  timelineSeekRequestId: number;
  /** Target seconds for the most recent timeline-time seek request. */
  timelineSeekTargetS: number;

  /** Refresh from `list_proxies`. */
  refresh: () => Promise<void>;
  /** Pick which asset plays. Resets playback state. */
  select: (stem: string | null) => void;
  /** Called by the player on `timeupdate` / scrub (source-time). */
  setTime: (t: number) => void;
  /** Called on `loadedmetadata` (source-time). */
  setDuration: (d: number) => void;
  /** Called on `play` / `pause` / `ended`. */
  setPlaying: (p: boolean) => void;
  /** Called by preview players when WebKit fails to load/play media. */
  setMediaError: (message: string | null) => void;
  /** External caller asks the player to seek to `t` seconds (source-time). */
  requestSeek: (t: number) => void;

  /** Called by the segmented player as it plays / scrubs (timeline-time). */
  setTimelineTime: (t: number) => void;
  /** Called by the MediaPane when the OTIO snapshot's duration changes. */
  setTimelineDuration: (d: number) => void;
  /** External caller asks the segmented player to seek (timeline-time). */
  requestTimelineSeek: (t: number) => void;
};

export const useMediaStore = create<MediaState>((set, get) => ({
  sources: [],
  proxies: [],
  selectedStem: null,
  currentTime: 0,
  durationS: 0,
  isPlaying: false,
  mediaError: null,
  seekRequestId: 0,
  seekTargetS: 0,
  timelineTime: 0,
  timelineDurationS: 0,
  timelineSeekRequestId: 0,
  timelineSeekTargetS: 0,

  refresh: async () => {
    try {
      const [sources, proxies] = await Promise.all([
        invoke<SourceMediaEntry[]>("list_source_media"),
        invoke<ProxyEntry[]>("list_proxies"),
      ]);
      set((state) => {
        // Keep the current selection if it still resolves against
        // either the proxies (post-transcode) or the sources (pre-
        // transcode); otherwise fall back to the first proxy, then
        // the first source's filename-minus-extension. The source
        // fallback matters when an asset's whisper sidecar lands
        // before its proxy — without it the transcript pane stays
        // empty even though the indexer is done.
        const stillThere =
          state.selectedStem !== null &&
          (proxies.some((p) => p.stem === state.selectedStem) ||
            sources.some((s) => stripExt(s.name) === state.selectedStem));
        const selectedStem = stillThere
          ? state.selectedStem
          : proxies[0]?.stem ?? (sources[0] ? stripExt(sources[0].name) : null);
        return { sources, proxies, selectedStem };
      });
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn("list_proxies failed", e);
    }
  },
  select: (stem) => {
    if (stem === null) {
      set({
        selectedStem: null,
        currentTime: 0,
        durationS: 0,
        isPlaying: false,
        mediaError: null,
      });
      return;
    }
    // Accept either a proxy stem (post-transcode) or a source
    // filename-without-extension (pre-transcode). Without this,
    // selecting a freshly-imported asset before its proxy has
    // finished transcoding would be silently refused.
    const { proxies, sources } = get();
    const resolves =
      proxies.some((p) => p.stem === stem) ||
      sources.some((s) => stripExt(s.name) === stem);
    if (resolves) {
      set({
        selectedStem: stem,
        currentTime: 0,
        durationS: 0,
        isPlaying: false,
        mediaError: null,
      });
    }
  },
  setTime: (t) => set({ currentTime: t }),
  setDuration: (d) => set({ durationS: d }),
  setPlaying: (p) => set({ isPlaying: p }),
  setMediaError: (message) => set({ mediaError: message }),
  requestSeek: (t) =>
    set((state) => ({
      seekRequestId: state.seekRequestId + 1,
      seekTargetS: Math.max(0, t),
    })),
  setTimelineTime: (t) =>
    set((state) => ({
      timelineTime: clampTimelineTime(t, state.timelineDurationS),
    })),
  setTimelineDuration: (d) =>
    set((state) => {
      const duration = Math.max(0, d);
      return {
        timelineDurationS: duration,
        timelineTime: clampTimelineTime(state.timelineTime, duration),
        timelineSeekTargetS: clampTimelineTime(state.timelineSeekTargetS, duration),
      };
    }),
  requestTimelineSeek: (t) =>
    set((state) => {
      const target = clampTimelineTime(t, state.timelineDurationS);
      return {
        timelineTime: target,
        timelineSeekRequestId: state.timelineSeekRequestId + 1,
        timelineSeekTargetS: target,
      };
    }),
}));

function clampTimelineTime(t: number, durationS: number): number {
  const safe = Number.isFinite(t) ? Math.max(0, t) : 0;
  return durationS > 0 ? Math.min(safe, durationS) : safe;
}

function stripExt(filename: string): string {
  const slash = Math.max(filename.lastIndexOf("/"), filename.lastIndexOf("\\"));
  const base = slash >= 0 ? filename.slice(slash + 1) : filename;
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(0, dot) : base;
}
