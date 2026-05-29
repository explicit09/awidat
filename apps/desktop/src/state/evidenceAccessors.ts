// evidenceAccessors — typed read accessors over indexer evidence.
//
// Wave 3 surfaces (Brief evidence chips B3, drill-down panels C1) need to
// answer "show me the data the indexer produced", not just "is the indexer
// ready". This file is the single place those surfaces look.
//
// Status today (Wave 3 F4 audit):
//
//   indexer        | on disk                            | loaded into FE state?       | accessor returns
//   ---------------|------------------------------------|-----------------------------|---------------------
//   transcripts    | index/whisper/*.json               | yes — useTranscriptStore    | EvidenceTranscriptSegment[]
//   speaker        | index/whisper/*.json (diarized)    | yes — useTranscriptStore    | EvidenceSpeaker[]
//   captions       | derivable from whisper             | yes — useTranscriptStore    | EvidenceCaption[]
//   scenes         | index/scenedetect/*.json           | NO  — only scene_count      | []  (gap)
//   silence        | .awidat/silences/*.json            | NO                          | []  (gap)
//   audio          | index/audio-energy/*.json          | NO                          | []  (gap)
//   face           | index/face/*.json                  | NO                          | []  (gap)
//   motion         | .awidat/motion/*.json              | NO                          | []  (gap)
//   color          | index/color-analysis/*.json        | NO                          | []  (gap)
//
// Gaps: scenes / silence / audio / face / motion / color have ready-state
// booleans only. To make them queryable we'd need a backend command that
// returns the parsed sidecars (per-asset arrays of scene boundaries,
// silence ranges, etc.). That's a follow-up; this layer surfaces what's
// reachable today and exports an availability map so B3 can render the
// chips that have data as clickable and the rest as greyed-out.
//
// Design rules:
//   - Every hook returns an array (or array-equivalent), never undefined.
//     Callers iterate without null-checks.
//   - Accessors don't trigger fetches. Whatever's in the stores is what
//     they return. Triggering loads is the responsibility of the surface
//     that owns the lifecycle (transcripts are loaded by the transcript
//     pane / segmented player setting `activeStem`).
//   - When data isn't loaded, return []. Use `useEvidenceAvailability()`
//     to know whether the empty array means "indexer didn't run" vs
//     "data not exposed to the frontend yet".

import {
  useIndexReadinessStore,
  type IndexReadinessSnapshot,
} from "./indexReadiness.ts";
import { useTranscriptStore } from "../transcript/store.ts";
import type { Transcript } from "../protocol";

// ---------------------------------------------------------------------------
// Per-evidence shapes — the canonical types the Brief / drill-down render.
// Keep these stable; they're the public API of this module.
// ---------------------------------------------------------------------------

/** One scene boundary from the scenedetect indexer. Not reachable today. */
export interface EvidenceScene {
  id: string;
  /** Source-time start of the shot, in seconds. */
  start_s: number;
  /** Source-time end of the shot, in seconds. */
  end_s: number;
  /** Optional filmstrip thumbnail rendered by the proxy pipeline. */
  thumbnail_path?: string;
}

/** One speaker summary from a diarized whisper sidecar. */
export interface EvidenceSpeaker {
  id: string;
  /** Display label — same as id today, kept as a separate field so a
   *  rename action (`set_speaker_label`) can land without breaking
   *  consumers. */
  label: string;
  /** Path to a representative portrait crop. Not produced today; reserved. */
  portrait_path?: string;
  /** How many segments are attributed to this speaker. */
  segment_count: number;
  /** Total seconds of speech attributed to this speaker. */
  total_speech_s: number;
}

/** One silence range from the silence indexer. Not reachable today. */
export interface EvidenceSilence {
  start_s: number;
  end_s: number;
  duration_s: number;
}

/** One transcript segment from the whisper sidecar. */
export interface EvidenceTranscriptSegment {
  id: string;
  speaker_id?: string;
  start_s: number;
  end_s: number;
  text: string;
}

/** One caption row — same time-range shape as a segment, but with the
 *  one-liner shaped for caption rendering. Derivable from the transcript;
 *  the cache key is separate so the agent can distinguish "transcript
 *  ready" from "captions exported as SRT". */
export interface EvidenceCaption {
  id: string;
  start_s: number;
  end_s: number;
  text: string;
}

/** One face-detection bounding box. Not reachable today. */
export interface EvidenceFace {
  id: string;
  /** Centre time of the frame the face was detected on, in source seconds. */
  at_s: number;
  /** Bounding box in source-frame pixels. */
  bbox: { x: number; y: number; w: number; h: number };
  /** Optional identity cluster id when face clustering is wired up. */
  identity_id?: string;
}

/** One motion-energy region. Not reachable today. */
export interface EvidenceMotionRegion {
  id: string;
  start_s: number;
  end_s: number;
  /** Normalized 0..1 motion intensity. */
  intensity: number;
}

/** One color statistic (per-shot or per-frame). Not reachable today. */
export interface EvidenceColorStat {
  id: string;
  start_s: number;
  end_s: number;
  /** Average luma, 0..1. */
  luma_mean?: number;
  /** Average saturation, 0..1. */
  saturation_mean?: number;
}

/** One audio-energy sample. Not reachable today. */
export interface EvidenceAudioSample {
  at_s: number;
  /** RMS magnitude, 0..1. */
  rms: number;
}

// ---------------------------------------------------------------------------
// Availability map — what's true on disk vs reachable from frontend code.
// ---------------------------------------------------------------------------

/**
 * Per-indexer "is the data actually queryable from the frontend right
 * now?" map. This is the answer B3 needs to decide which evidence chip
 * is clickable vs greyed-out.
 *
 * The booleans here combine two signals:
 *   1. Did the indexer produce output? (`IndexReadinessSnapshot.<kind>`)
 *   2. Do we have a frontend accessor that returns the parsed data?
 *
 * Today only `transcripts`, `speakers`, and `captions` can be both true
 * (via the whisper sidecar that the transcript store already loads).
 * Every other indexer flips to `false` even when the on-disk sidecar
 * exists, because the frontend hasn't loaded it.
 */
export interface EvidenceAvailability {
  scenes: boolean;
  speakers: boolean;
  silences: boolean;
  transcript: boolean;
  captions: boolean;
  color: boolean;
  motion: boolean;
  faces: boolean;
  audio: boolean;
}

const ZERO_AVAILABILITY: EvidenceAvailability = {
  scenes: false,
  speakers: false,
  silences: false,
  transcript: false,
  captions: false,
  color: false,
  motion: false,
  faces: false,
  audio: false,
};

/**
 * Snapshot-shaped availability. Use the hook form (`useEvidenceAvailability`)
 * for React components; this pure helper is for tests and non-React
 * consumers.
 */
export function evidenceAvailabilityFromState(args: {
  snapshot: IndexReadinessSnapshot | undefined;
  loadedTranscript: Transcript | null;
}): EvidenceAvailability {
  const { snapshot, loadedTranscript } = args;
  if (!snapshot) return ZERO_AVAILABILITY;

  // Transcript / speakers / captions are reachable iff a transcript is
  // loaded into the store *and* the readiness snapshot agrees the
  // indexer ran. We require both so a stale store entry doesn't claim
  // "transcript available" after the user closed the project.
  const transcriptReachable =
    snapshot.transcripts && loadedTranscript !== null;
  const speakersReachable =
    snapshot.speaker &&
    loadedTranscript !== null &&
    loadedTranscript.diarized &&
    loadedTranscript.speakers.length > 0;
  const captionsReachable = snapshot.captions && loadedTranscript !== null;

  return {
    transcript: transcriptReachable,
    speakers: speakersReachable,
    captions: captionsReachable,
    // The remaining six are "ready on disk but not exposed to the
    // frontend today". Until a backend accessor lands, callers see
    // these as false and treat the chip as greyed-out.
    scenes: false,
    silences: false,
    color: false,
    motion: false,
    faces: false,
    audio: false,
  };
}

// ---------------------------------------------------------------------------
// Internal helpers — read the active transcript out of useTranscriptStore.
// ---------------------------------------------------------------------------

/**
 * The transcript that's currently displayed in the transcript pane, if
 * any. Read straight from useTranscriptStore so accessors stay in sync
 * with what the user is looking at.
 *
 * Exposed for tests so they can hook the store via setState.
 */
export function activeTranscript(): Transcript | null {
  const state = useTranscriptStore.getState();
  if (!state.activeStem) return null;
  const entry = state.byStem[state.activeStem];
  if (!entry || entry.state !== "loaded") return null;
  return entry.transcript;
}

// ---------------------------------------------------------------------------
// Selectors — pure projections over store state. Tests and non-React
// callers use these directly; the React hooks below wrap them with a
// store subscription so components re-render when the data changes.
//
// Selectors take "what's already in the stores" and return a typed array.
// They never trigger fetches.
// ---------------------------------------------------------------------------

export function selectScenes(): EvidenceScene[] {
  // Gap: scene boundaries aren't loaded into the frontend today. The
  // Rust `index_readiness` command reports `scene_count` but the per-
  // shot list lives in `index/scenedetect/*.json` on disk and isn't
  // exposed via any Tauri command.
  return [];
}

export function selectSpeakers(): EvidenceSpeaker[] {
  const t = activeTranscript();
  if (!t || !t.diarized) return [];
  const segmentCounts = new Map<string, number>();
  for (const seg of t.segments) {
    if (!seg.speaker_id) continue;
    segmentCounts.set(seg.speaker_id, (segmentCounts.get(seg.speaker_id) ?? 0) + 1);
  }
  return t.speakers.map((sp) => ({
    id: sp.id,
    label: sp.id,
    segment_count: segmentCounts.get(sp.id) ?? 0,
    total_speech_s: sp.total_speech_s,
  }));
}

export function selectSilences(): EvidenceSilence[] {
  // Gap: `.awidat/silences/` sidecars aren't loaded into the frontend.
  return [];
}

export function selectTranscriptSegments(): EvidenceTranscriptSegment[] {
  const t = activeTranscript();
  if (!t) return [];
  return t.segments.map((seg, idx) => ({
    id: `${t.asset_stem}-seg-${idx}`,
    speaker_id: seg.speaker_id ?? undefined,
    start_s: seg.start_s,
    end_s: seg.end_s,
    text: seg.text,
  }));
}

export function selectCaptions(): EvidenceCaption[] {
  // Derived from the same whisper sidecar; the captions indexer ships
  // SRT but the row data is identical.
  return selectTranscriptSegments().map((seg) => ({
    id: seg.id.replace(/-seg-/, "-cap-"),
    start_s: seg.start_s,
    end_s: seg.end_s,
    text: seg.text,
  }));
}

export function selectColorStats(): EvidenceColorStat[] {
  // Gap: `index/color-analysis/` sidecars aren't loaded into the frontend.
  return [];
}

export function selectMotionRegions(): EvidenceMotionRegion[] {
  // Gap: `.awidat/motion/` sidecars aren't loaded into the frontend.
  return [];
}

export function selectFaces(): EvidenceFace[] {
  // Gap: `index/face/` sidecars aren't loaded into the frontend.
  return [];
}

export function selectAudioSamples(): EvidenceAudioSample[] {
  // Gap: `index/audio-energy/` analysis sidecars aren't loaded into the
  // frontend. Per-asset waveform PEAKS (different signal) are loaded for
  // the timeline strip via `TimelineItem.waveform_path`.
  return [];
}

export function selectEvidenceAvailability(): EvidenceAvailability {
  const snapshot = useIndexReadinessStore.getState().snapshot;
  return evidenceAvailabilityFromState({
    snapshot,
    loadedTranscript: activeTranscript(),
  });
}

// ---------------------------------------------------------------------------
// React hooks — wrap the selectors with store subscriptions so components
// re-render when the underlying data changes.
//
// Each hook subscribes to the minimal slice of store state the selector
// depends on. The selector itself does the projection. This keeps the
// pure projection logic testable without React renderer setup.
// ---------------------------------------------------------------------------

/** Scenes from the scenedetect indexer. Returns [] today — gap documented. */
export function useScenes(): EvidenceScene[] {
  // Subscribe so a future backend that ships per-scene data triggers
  // re-renders automatically.
  useIndexReadinessStore((s) => s.snapshot);
  return selectScenes();
}

/** Speakers from the active diarized transcript. */
export function useSpeakers(): EvidenceSpeaker[] {
  useTranscriptStore((s) => s.activeStem);
  useTranscriptStore((s) => s.byStem);
  return selectSpeakers();
}

/** Silence ranges. Returns [] today — gap documented. */
export function useSilences(): EvidenceSilence[] {
  useIndexReadinessStore((s) => s.snapshot);
  return selectSilences();
}

/** Transcript segments from the active whisper sidecar. */
export function useTranscriptSegments(): EvidenceTranscriptSegment[] {
  useTranscriptStore((s) => s.activeStem);
  useTranscriptStore((s) => s.byStem);
  return selectTranscriptSegments();
}

/** Caption rows derived from the active whisper sidecar. */
export function useCaptions(): EvidenceCaption[] {
  useTranscriptStore((s) => s.activeStem);
  useTranscriptStore((s) => s.byStem);
  return selectCaptions();
}

/** Color stats. Returns [] today — gap documented. */
export function useColorStats(): EvidenceColorStat[] {
  useIndexReadinessStore((s) => s.snapshot);
  return selectColorStats();
}

/** Motion regions. Returns [] today — gap documented. */
export function useMotionRegions(): EvidenceMotionRegion[] {
  useIndexReadinessStore((s) => s.snapshot);
  return selectMotionRegions();
}

/** Face bounding boxes. Returns [] today — gap documented. */
export function useFaces(): EvidenceFace[] {
  useIndexReadinessStore((s) => s.snapshot);
  return selectFaces();
}

/** Audio-energy samples. Returns [] today — gap documented. */
export function useAudioSamples(): EvidenceAudioSample[] {
  useIndexReadinessStore((s) => s.snapshot);
  return selectAudioSamples();
}

/**
 * Combined "which evidence chips have data?" map. Wave 3 B3 reads this
 * to decide which chips render as clickable vs greyed-out.
 */
export function useEvidenceAvailability(): EvidenceAvailability {
  useIndexReadinessStore((s) => s.snapshot);
  useTranscriptStore((s) => s.activeStem);
  useTranscriptStore((s) => s.byStem);
  return selectEvidenceAvailability();
}
