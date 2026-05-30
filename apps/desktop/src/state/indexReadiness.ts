// useIndexReadinessStore — single shared holder for the `index_readiness`
// Tauri command snapshot.
//
// Background: `App.tsx` invokes `index_readiness` on boot and after every
// indexer run, and kept the result in local React state. That worked for
// the cockpit slate and the footer's ad-hoc consumers, but Wave 3 surfaces
// (Brief evidence chips, drill-down panels) and the new `evidenceAccessors`
// module need to subscribe to the same snapshot without prop-drilling
// through five layers of shell components.
//
// This store is intentionally tiny — it owns the snapshot + a setter and
// nothing else. App.tsx is still the one calling `invoke("index_readiness")`;
// it just also writes the result here so accessors can read it.
//
// The shape mirrors the Rust `IndexReadinessSnapshot` (commands/index.rs):
// nine indexer booleans + ready_count + scene_count. No actual evidence
// payloads (scene boundaries, silence ranges, etc.) live here — those are
// not exposed to the frontend today. The `evidenceAccessors` module
// documents the gap explicitly.

import { create } from "zustand";

/**
 * Mirror of the Rust `IndexReadinessSnapshot` struct in
 * `apps/desktop/src-tauri/src/commands/index.rs`. Each boolean is true iff
 * the indexer has produced at least one sidecar under the project root.
 *
 * Note: `speaker` here means "any whisper sidecar has `data.diarized: true`";
 * the actual speaker labels live inside the whisper sidecar body, not in
 * a dedicated dir.
 */
export interface IndexReadinessSnapshot {
  transcripts: boolean;
  scenes: boolean;
  audio: boolean;
  face: boolean;
  motion: boolean;
  color: boolean;
  silence: boolean;
  speaker: boolean;
  captions: boolean;
  /** Count of indexers with ready_count === true above. */
  ready_count: number;
  /**
   * Total `shots` across every scenedetect sidecar. `0` is ambiguous —
   * the indexer either hasn't run (combine with `scenes`) or ran and
   * found no cuts.
   */
  scene_count: number;
}

interface IndexReadinessState {
  /** Latest snapshot from the backend; undefined until the first load. */
  snapshot: IndexReadinessSnapshot | undefined;
  /** Replace the snapshot. Pass `undefined` on project unload. */
  setSnapshot: (snapshot: IndexReadinessSnapshot | undefined) => void;
}

export const useIndexReadinessStore = create<IndexReadinessState>((set) => ({
  snapshot: undefined,
  setSnapshot: (snapshot) => set({ snapshot }),
}));
