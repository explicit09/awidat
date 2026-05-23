// Transcript pane state — fetched per-stem, cached on the frontend
// in addition to the backend's parse cache. Two layers because the
// backend cache survives across tab toggles within one process,
// but Zustand is what React subscribes to — the component needs
// state-shaped data, not a fire-and-forget cache.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { Transcript } from "../protocol";

/** Per-stem load state. `"loading"` means the fetch is in flight,
 *  `"missing"` means the backend returned None (no sidecar yet —
 *  asset transcoded but not whisper-indexed, or the indexer
 *  failed). `Transcript` is the loaded value. */
export type TranscriptState =
  | { state: "loading" }
  | { state: "missing" }
  | { state: "loaded"; transcript: Transcript };

type State = {
  /** Stem → loaded / loading / missing. */
  byStem: Record<string, TranscriptState>;
  /** Currently-displayed transcript stem (mirrors the active media
   *  pane / segmented player). */
  activeStem: string | null;
  /** Word-level selection range (for drag-select-to-delete in 6.7).
   *  Null when nothing is selected. */
  selection: SelectionRange | null;

  /** Kick off a fetch for `stem` if not already loading or loaded.
   *  No-op if `stem` is null. */
  load: (stem: string | null) => Promise<void>;
  /** Force re-fetch for `stem`, bypassing the cache. Used after the
   *  user renames a speaker via the inline speaker label — the backend
   *  already invalidates its own cache, but we need to wipe the front-
   *  end byStem entry too so the next render sees the new labels. */
  reload: (stem: string) => Promise<void>;
  /** Switch the active stem. Triggers a load if needed. */
  setActiveStem: (stem: string | null) => void;
  /** Clear all cached transcripts (called on whisper-job Completed
   *  events from the agent store). */
  clearCache: () => void;
  /** Selection actions (used in 6.7). */
  setSelection: (sel: SelectionRange | null) => void;
};

export type SelectionRange = {
  /** Stem the selection belongs to (anchors the range to a specific
   *  transcript so a stem-switch clears it). */
  stem: string;
  /** Index of the first selected word in the transcript's words[]. */
  startWordIdx: number;
  /** Index of the last selected word (inclusive). */
  endWordIdx: number;
};

export const useTranscriptStore = create<State>((set, get) => ({
  byStem: {},
  activeStem: null,
  selection: null,

  load: async (stem) => {
    if (!stem) return;
    const existing = get().byStem[stem];
    if (existing && existing.state !== "missing") return;
    set((s) => ({
      byStem: { ...s.byStem, [stem]: { state: "loading" } },
    }));
    try {
      const result = await invoke<Transcript | null>("read_transcript", {
        stem,
      });
      set((s) => ({
        byStem: {
          ...s.byStem,
          [stem]:
            result === null
              ? { state: "missing" }
              : { state: "loaded", transcript: result },
        },
      }));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("read_transcript failed", stem, err);
      set((s) => ({
        byStem: { ...s.byStem, [stem]: { state: "missing" } },
      }));
    }
  },

  reload: async (stem) => {
    // Drop the cached entry so `load` doesn't short-circuit.
    set((s) => {
      const next = { ...s.byStem };
      delete next[stem];
      return { byStem: next };
    });
    await get().load(stem);
  },

  setActiveStem: (stem) => {
    set({ activeStem: stem, selection: null });
    if (stem) void get().load(stem);
  },

  clearCache: () =>
    set((s) => ({
      // Wipe everything except the active stem's loading state, which
      // we re-trigger to repopulate. Avoids a flicker where the user
      // sees "no transcript" for one paint after the indexer
      // refreshes the sidecar.
      byStem: {},
      selection:
        s.selection && s.activeStem !== null && s.selection.stem === s.activeStem
          ? s.selection
          : null,
    })),

  setSelection: (sel) => set({ selection: sel }),
}));
