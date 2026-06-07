// evidenceAccessors — typed read accessors over indexer evidence.
//
// Wave 3 surfaces (Brief evidence chips B3, drill-down panels C1) need to
// answer "show me the data the indexer produced", not just "is the indexer
// ready". This file is the single place those surfaces look.
//
// Wave 3 C1 closed the previous gap: the six accessors that used to
// return [] now invoke six Tauri reader commands
// (`read_scenes`, `read_silences`, `read_audio_samples`, `read_faces`,
// `read_motion_regions`, `read_color_stats`). Each command resolves the
// sidecar against the project root and returns parsed rows; an absent
// sidecar comes back as []. Results live in `useEvidenceDataStore`,
// keyed by `(project_path, stem)` so a project switch or active-stem
// switch transparently re-fetches.
//
// Per-accessor coverage today:
//
//   indexer        | sidecar path                          | accessor          | type
//   ---------------|---------------------------------------|-------------------|----------------------
//   transcripts    | index/whisper/<asset>.json            | useTranscriptStore| EvidenceTranscriptSegment[]
//   speaker        | index/whisper/<asset>.json            | useTranscriptStore| EvidenceSpeaker[]
//   captions       | derivable from whisper                | useTranscriptStore| EvidenceCaption[]
//   scenes         | index/scenedetect/<asset>.json        | read_scenes       | EvidenceScene[]
//   silence        | .montage/silences/<stem>-<hash>.json   | read_silences     | EvidenceSilence[]
//   audio          | index/audio-energy/<asset>.json       | read_audio_samples| EvidenceAudioSample[]
//   face           | index/face/<asset>.json               | read_faces        | EvidenceFace[]
//   motion         | .montage/motion/<stem>-<hash>.json     | read_motion_regions| EvidenceMotionRegion[]
//   color          | index/color-analysis/<asset>.json     | read_color_stats  | EvidenceColorStat[]
//
// Design rules (kept stable from B3):
//   - Every hook returns an array (or array-equivalent), never undefined.
//     Callers iterate without null-checks.
//   - Hooks DO trigger lazy loads as of C1 (single `invoke` per
//     project+stem pair). Triggering loads is now a side-effect of
//     reading; we accept the tradeoff so consumers don't need to thread
//     `useEffect` plumbing through every drill-down.
//   - When data isn't loaded, return []. Use `useEvidenceAvailability()`
//     to know whether the empty array means "indexer didn't run" vs
//     "still loading from disk".

import { invoke, isTauri } from "@tauri-apps/api/core";
import { create } from "zustand";
import { useProjectStore } from "../app/state.ts";
import {
  useIndexReadinessStore,
  type IndexReadinessSnapshot,
} from "./indexReadiness.ts";
import { useEpisodesStore } from "./episodes.ts";
import { useTranscriptStore } from "../transcript/store.ts";
import type { Transcript } from "../protocol";

// ---------------------------------------------------------------------------
// Per-evidence shapes — the canonical types the Brief / drill-down render.
// Keep these stable; they're the public API of this module.
// ---------------------------------------------------------------------------

/** One scene boundary from the scenedetect indexer. */
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

/** One silence range from the silence indexer. */
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

/** One face-detection bounding box. */
export interface EvidenceFace {
  id: string;
  /** Centre time of the frame the face was detected on, in source seconds. */
  at_s: number;
  /** Bounding box in source-frame pixels. */
  bbox: { x: number; y: number; w: number; h: number };
  /** Optional identity cluster id when face clustering is wired up. */
  identity_id?: string;
}

/** One motion-energy region. */
export interface EvidenceMotionRegion {
  id: string;
  start_s: number;
  end_s: number;
  /** Normalized 0..1 motion intensity. */
  intensity: number;
}

/** One color statistic (per-shot or per-frame). */
export interface EvidenceColorStat {
  id: string;
  start_s: number;
  end_s: number;
  /** Average luma, 0..1. */
  luma_mean?: number;
  /** Average saturation, 0..1. */
  saturation_mean?: number;
}

/** One audio-energy sample. */
export interface EvidenceAudioSample {
  at_s: number;
  /** RMS magnitude, 0..1. */
  rms: number;
}

/** One episode span from origin's first-class episodes feature.
 *
 *  Source: `get_project_episodes` Tauri command (reads OTIO
 *  metadata.montage.episodes). App.tsx owns the fetch and mirrors the
 *  result into `useEpisodesStore`; this accessor reads from that store
 *  so it doesn't need to invoke. */
export interface EvidenceEpisode {
  id: string;
  name: string;
  order: number;
  startS: number;
  endS: number;
  durationS: number;
  confidence: number;
  status: "accepted" | "review_needed" | "rejected";
  evidenceCount: number;
}

// ---------------------------------------------------------------------------
// Lazy-loading store — caches per (project_path, stem) tuple.
//
// We picked lazy over eager because the indexer fan-out can be large
// (a 90-minute interview yields ~54000 audio windows) and the user
// rarely opens every drill-down. The first read of an accessor for a
// given project triggers the fetch; subsequent reads hit the cached
// array. `clear()` is called on project change.
// ---------------------------------------------------------------------------

type EvidenceKind =
  | "scenes"
  | "silences"
  | "audio"
  | "faces"
  | "motion"
  | "color";

type Cached<T> = { kind: "loaded"; data: T[] } | { kind: "loading" };

interface EvidenceDataState {
  /** `${project}::${stem}::${kind}` → cached rows or in-flight flag. */
  cache: Record<string, Cached<unknown>>;
  /** Last project key the cache was scoped to. When this changes the
   *  cache is wiped — same pattern as transcriptStore.clearCache. */
  scope: string | null;
  /** Drop everything cached (project switched, indexer re-ran, etc). */
  clear: () => void;
  /** Scope the cache to `project`. Returns true iff the scope changed. */
  setScope: (project: string | null) => boolean;
  /** Internal: replace one cache entry. */
  set: (key: string, value: Cached<unknown>) => void;
}

const useEvidenceDataStore = create<EvidenceDataState>((set) => ({
  cache: {},
  scope: null,
  clear: () => set({ cache: {} }),
  setScope: (project) => {
    let changed = false;
    set((s) => {
      if (s.scope === project) return s;
      changed = true;
      return { scope: project, cache: {} };
    });
    return changed;
  },
  set: (key, value) =>
    set((s) => ({ cache: { ...s.cache, [key]: value } })),
}));

function cacheKey(project: string, stem: string, kind: EvidenceKind): string {
  return `${project}::${stem}::${kind}`;
}

/**
 * Trigger a lazy load for `kind` against `(project, stem)`. No-op if
 * the entry is already cached or in flight. Outside Tauri (jest /
 * vitest / Node) we never invoke; tests seed the store directly.
 */
function ensureLoaded<T>(
  project: string | null,
  stem: string | null,
  kind: EvidenceKind,
  command: string,
): T[] {
  if (!project || !stem) return [];
  const store = useEvidenceDataStore.getState();
  // Keep the cache scoped to one project; if the project changed,
  // wipe before checking the per-key entry.
  store.setScope(project);
  const key = cacheKey(project, stem, kind);
  const hit = store.cache[key];
  if (hit) {
    return hit.kind === "loaded" ? (hit.data as T[]) : [];
  }
  if (!isTauri()) return [];
  // Mark loading first so re-renders during fetch don't queue parallel
  // invokes for the same key.
  store.set(key, { kind: "loading" });
  invoke<T[]>(command, { projectPath: project, stem })
    .then((rows) => {
      useEvidenceDataStore
        .getState()
        .set(key, { kind: "loaded", data: rows ?? [] });
    })
    .catch((err) => {
      // eslint-disable-next-line no-console
      console.warn(`${command} failed for ${stem}`, err);
      // Treat failures as "no data" rather than retrying forever.
      useEvidenceDataStore.getState().set(key, { kind: "loaded", data: [] });
    });
  return [];
}

/**
 * Test-only helper. Seeds the cache so the selectors / availability
 * map report data without invoking Tauri. Use with care — production
 * code should never call this.
 */
export function __setEvidenceDataForTests<T>(
  project: string,
  stem: string,
  kind: EvidenceKind,
  data: T[],
): void {
  useEvidenceDataStore.getState().setScope(project);
  useEvidenceDataStore
    .getState()
    .set(cacheKey(project, stem, kind), { kind: "loaded", data });
}

/** Test-only — clear the lazy-load cache between cases. */
export function __resetEvidenceDataForTests(): void {
  useEvidenceDataStore.setState({ cache: {}, scope: null });
}

// ---------------------------------------------------------------------------
// Availability map — what's true on disk vs reachable from frontend code.
// ---------------------------------------------------------------------------

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
  episodes: boolean;
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
  episodes: false,
};

/**
 * Snapshot-shaped availability. Use the hook form (`useEvidenceAvailability`)
 * for React components; this pure helper is for tests and non-React
 * consumers.
 *
 * Accepts optional per-kind loaded counts so consumers can flip a chip
 * from "indexer-ready but empty" to "has data" without re-querying the
 * cache. When a count is unset we fall back to the snapshot boolean —
 * keeping the contract that "indexer ran, sidecar present" is the
 * default truth.
 */
export function evidenceAvailabilityFromState(args: {
  snapshot: IndexReadinessSnapshot | undefined;
  loadedTranscript: Transcript | null;
  loadedCounts?: Partial<Record<EvidenceKind, number>>;
  /** Number of episodes in the episodes store. Episodes don't live in
   *  the readiness snapshot (they're stamped on the timeline, not the
   *  indexer fan-out), so the count flows in as a separate signal. */
  episodeCount?: number;
}): EvidenceAvailability {
  const { snapshot, loadedTranscript, loadedCounts, episodeCount } = args;
  if (!snapshot) {
    // Episodes are independent of the indexer readiness snapshot — if
    // the timeline has episodes stamped, surface them even before the
    // indexer load completes.
    if ((episodeCount ?? 0) > 0) {
      return { ...ZERO_AVAILABILITY, episodes: true };
    }
    return ZERO_AVAILABILITY;
  }

  const transcriptReachable =
    snapshot.transcripts && loadedTranscript !== null;
  const speakersReachable =
    snapshot.speaker &&
    loadedTranscript !== null &&
    loadedTranscript.diarized &&
    loadedTranscript.speakers.length > 0;
  const captionsReachable = snapshot.captions && loadedTranscript !== null;

  // For the six lazy accessors: if a loaded count is supplied, prefer
  // it — `> 0` proves the data is reachable. Otherwise fall back to
  // the readiness snapshot ("sidecar exists" is a reasonable proxy
  // when we haven't fetched yet).
  function flagFor(kind: EvidenceKind, snapshotFlag: boolean): boolean {
    const count = loadedCounts?.[kind];
    if (count !== undefined) return count > 0;
    return snapshotFlag;
  }

  return {
    transcript: transcriptReachable,
    speakers: speakersReachable,
    captions: captionsReachable,
    scenes: flagFor("scenes", snapshot.scenes),
    silences: flagFor("silences", snapshot.silence),
    color: flagFor("color", snapshot.color),
    motion: flagFor("motion", snapshot.motion),
    faces: flagFor("faces", snapshot.face),
    audio: flagFor("audio", snapshot.audio),
    episodes: (episodeCount ?? 0) > 0,
  };
}

// ---------------------------------------------------------------------------
// Internal helpers — read the active transcript out of useTranscriptStore.
// ---------------------------------------------------------------------------

export function activeTranscript(): Transcript | null {
  const state = useTranscriptStore.getState();
  if (!state.activeStem) return null;
  const entry = state.byStem[state.activeStem];
  if (!entry || entry.state !== "loaded") return null;
  return entry.transcript;
}

function activeProject(): string | null {
  return useProjectStore.getState().current;
}

function activeStem(): string | null {
  return useTranscriptStore.getState().activeStem;
}

function readCached<T>(
  project: string | null,
  stem: string | null,
  kind: EvidenceKind,
): T[] {
  if (!project || !stem) return [];
  const hit = useEvidenceDataStore.getState().cache[
    cacheKey(project, stem, kind)
  ];
  if (!hit || hit.kind !== "loaded") return [];
  return hit.data as T[];
}

// ---------------------------------------------------------------------------
// Selectors — pure projections over store state. Tests and non-React
// callers use these directly; the React hooks below wrap them with a
// store subscription so components re-render when the data changes.
// ---------------------------------------------------------------------------

export function selectScenes(): EvidenceScene[] {
  return readCached<EvidenceScene>(activeProject(), activeStem(), "scenes");
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
  return readCached<EvidenceSilence>(activeProject(), activeStem(), "silences");
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
  return selectTranscriptSegments().map((seg) => ({
    id: seg.id.replace(/-seg-/, "-cap-"),
    start_s: seg.start_s,
    end_s: seg.end_s,
    text: seg.text,
  }));
}

export function selectColorStats(): EvidenceColorStat[] {
  return readCached<EvidenceColorStat>(activeProject(), activeStem(), "color");
}

export function selectMotionRegions(): EvidenceMotionRegion[] {
  return readCached<EvidenceMotionRegion>(activeProject(), activeStem(), "motion");
}

export function selectFaces(): EvidenceFace[] {
  return readCached<EvidenceFace>(activeProject(), activeStem(), "faces");
}

export function selectAudioSamples(): EvidenceAudioSample[] {
  return readCached<EvidenceAudioSample>(activeProject(), activeStem(), "audio");
}

/**
 * Episodes are stamped on the OTIO timeline (not produced by the
 * indexer fan-out) so they live in their own store. App.tsx owns the
 * fetch via `get_project_episodes`; this selector projects the
 * snapshot into the unified `EvidenceEpisode` shape and orders the
 * rows by `order` (then `startS` as a tiebreak) so callers don't have
 * to sort.
 */
export function selectEpisodes(): EvidenceEpisode[] {
  const snapshot = useEpisodesStore.getState().snapshot;
  if (!snapshot || snapshot.episodes.length === 0) return [];
  return snapshot.episodes
    .map((episode) => ({
      id: episode.id,
      name: episode.name,
      order: episode.order,
      startS: episode.startS,
      endS: episode.endS,
      durationS: episode.durationS,
      confidence: episode.confidence,
      status: episode.status,
      evidenceCount: episode.evidenceCount ?? 0,
    }))
    .sort((a, b) => a.order - b.order || a.startS - b.startS);
}

export function selectEvidenceAvailability(): EvidenceAvailability {
  const snapshot = useIndexReadinessStore.getState().snapshot;
  const project = activeProject();
  const stem = activeStem();
  const loadedCounts: Partial<Record<EvidenceKind, number>> = {};
  if (project && stem) {
    const cache = useEvidenceDataStore.getState().cache;
    const kinds: EvidenceKind[] = [
      "scenes",
      "silences",
      "audio",
      "faces",
      "motion",
      "color",
    ];
    for (const kind of kinds) {
      const hit = cache[cacheKey(project, stem, kind)];
      if (hit && hit.kind === "loaded") {
        loadedCounts[kind] = (hit.data as unknown[]).length;
      }
    }
  }
  const episodeCount =
    useEpisodesStore.getState().snapshot?.episodes.length ?? 0;
  return evidenceAvailabilityFromState({
    snapshot,
    loadedTranscript: activeTranscript(),
    loadedCounts,
    episodeCount,
  });
}

// ---------------------------------------------------------------------------
// React hooks — wrap the selectors with store subscriptions so components
// re-render when the underlying data changes. The lazy accessors also
// kick off a fetch on first read.
// ---------------------------------------------------------------------------

export function useScenes(): EvidenceScene[] {
  useIndexReadinessStore((s) => s.snapshot);
  useEvidenceDataStore((s) => s.cache);
  useProjectStore((s) => s.current);
  useTranscriptStore((s) => s.activeStem);
  ensureLoaded<EvidenceScene>(activeProject(), activeStem(), "scenes", "read_scenes");
  return selectScenes();
}

export function useSpeakers(): EvidenceSpeaker[] {
  useTranscriptStore((s) => s.activeStem);
  useTranscriptStore((s) => s.byStem);
  return selectSpeakers();
}

export function useSilences(): EvidenceSilence[] {
  useIndexReadinessStore((s) => s.snapshot);
  useEvidenceDataStore((s) => s.cache);
  useProjectStore((s) => s.current);
  useTranscriptStore((s) => s.activeStem);
  ensureLoaded<EvidenceSilence>(activeProject(), activeStem(), "silences", "read_silences");
  return selectSilences();
}

export function useTranscriptSegments(): EvidenceTranscriptSegment[] {
  useTranscriptStore((s) => s.activeStem);
  useTranscriptStore((s) => s.byStem);
  return selectTranscriptSegments();
}

export function useCaptions(): EvidenceCaption[] {
  useTranscriptStore((s) => s.activeStem);
  useTranscriptStore((s) => s.byStem);
  return selectCaptions();
}

export function useColorStats(): EvidenceColorStat[] {
  useIndexReadinessStore((s) => s.snapshot);
  useEvidenceDataStore((s) => s.cache);
  useProjectStore((s) => s.current);
  useTranscriptStore((s) => s.activeStem);
  ensureLoaded<EvidenceColorStat>(activeProject(), activeStem(), "color", "read_color_stats");
  return selectColorStats();
}

export function useMotionRegions(): EvidenceMotionRegion[] {
  useIndexReadinessStore((s) => s.snapshot);
  useEvidenceDataStore((s) => s.cache);
  useProjectStore((s) => s.current);
  useTranscriptStore((s) => s.activeStem);
  ensureLoaded<EvidenceMotionRegion>(activeProject(), activeStem(), "motion", "read_motion_regions");
  return selectMotionRegions();
}

export function useFaces(): EvidenceFace[] {
  useIndexReadinessStore((s) => s.snapshot);
  useEvidenceDataStore((s) => s.cache);
  useProjectStore((s) => s.current);
  useTranscriptStore((s) => s.activeStem);
  ensureLoaded<EvidenceFace>(activeProject(), activeStem(), "faces", "read_faces");
  return selectFaces();
}

export function useAudioSamples(): EvidenceAudioSample[] {
  useIndexReadinessStore((s) => s.snapshot);
  useEvidenceDataStore((s) => s.cache);
  useProjectStore((s) => s.current);
  useTranscriptStore((s) => s.activeStem);
  ensureLoaded<EvidenceAudioSample>(activeProject(), activeStem(), "audio", "read_audio_samples");
  return selectAudioSamples();
}

export function useEpisodes(): EvidenceEpisode[] {
  // Subscribe to the episodes store so the component re-renders when
  // App.tsx writes a new snapshot. App.tsx owns the fetch — this hook
  // never invokes.
  useEpisodesStore((s) => s.snapshot);
  return selectEpisodes();
}

export function useEvidenceAvailability(): EvidenceAvailability {
  useIndexReadinessStore((s) => s.snapshot);
  useEvidenceDataStore((s) => s.cache);
  useProjectStore((s) => s.current);
  useTranscriptStore((s) => s.activeStem);
  useTranscriptStore((s) => s.byStem);
  useEpisodesStore((s) => s.snapshot);
  return selectEvidenceAvailability();
}
