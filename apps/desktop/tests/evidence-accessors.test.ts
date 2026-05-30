// Tests for state/evidenceAccessors — the typed accessor layer over the
// IndexReadinessSnapshot + the transcript store.
//
// These are pure-logic tests: no React renderer. The module exports both
// React hooks (`useScenes`, `useEvidenceAvailability`, …) and pure
// selectors (`selectScenes`, …). The hooks delegate to the selectors after
// subscribing to the store; we test the selectors directly so we don't
// need a renderer.
//
// We exercise the contract by:
//   1. Seeding useIndexReadinessStore with a snapshot
//   2. Seeding useTranscriptStore.byStem with a fake Transcript
//   3. Calling the selectors (which read store state on invocation)
//
// What we lock in:
//   - selectTranscriptSegments + selectCaptions return the segments/captions
//     from the active transcript
//   - selectSpeakers projects diarized speakers + correctly counts segments
//     per speaker
//   - All other selectors return [] (gap documented in evidenceAccessors.ts)
//   - evidenceAvailabilityFromState mirrors what's actually reachable
//   - Switching activeStem to a stem with no loaded transcript drops
//     everything back to []
//   - A snapshot with `transcripts: true` but no loaded Transcript still
//     reports `transcript: false` — we require BOTH signals

import { strict as assert } from "node:assert";
import {
  __resetEvidenceDataForTests,
  __setEvidenceDataForTests,
  activeTranscript,
  evidenceAvailabilityFromState,
  selectAudioSamples,
  selectCaptions,
  selectColorStats,
  selectEpisodes,
  selectEvidenceAvailability,
  selectFaces,
  selectMotionRegions,
  selectScenes,
  selectSilences,
  selectSpeakers,
  selectTranscriptSegments,
  type EvidenceScene,
} from "../src/state/evidenceAccessors.ts";
import {
  useIndexReadinessStore,
  type IndexReadinessSnapshot,
} from "../src/state/indexReadiness.ts";
import { useEpisodesStore, type EpisodesSnapshot } from "../src/state/episodes.ts";
import { useProjectStore } from "../src/app/state.ts";
import { useTranscriptStore } from "../src/transcript/store.ts";
import type { Transcript } from "../src/protocol";

function resetStores(): void {
  useIndexReadinessStore.setState({ snapshot: undefined });
  useTranscriptStore.setState({
    byStem: {},
    activeStem: null,
    selection: null,
  });
  useProjectStore.setState({ current: null });
  useEpisodesStore.setState({ snapshot: undefined });
  __resetEvidenceDataForTests();
}

function episodes(rows: EpisodesSnapshot["episodes"]): EpisodesSnapshot {
  return {
    total: rows.length,
    accepted: rows.filter((r) => r.status === "accepted").length,
    reviewNeeded: rows.filter((r) => r.status === "review_needed").length,
    rejected: rows.filter((r) => r.status === "rejected").length,
    episodes: rows,
  };
}

function snapshot(overrides: Partial<IndexReadinessSnapshot> = {}): IndexReadinessSnapshot {
  return {
    transcripts: true,
    scenes: true,
    audio: true,
    face: true,
    motion: true,
    color: true,
    silence: true,
    speaker: true,
    captions: true,
    ready_count: 9,
    scene_count: 42,
    ...overrides,
  };
}

function transcript(opts: {
  stem: string;
  diarized?: boolean;
  segments?: Array<{
    text: string;
    start_s: number;
    end_s: number;
    speaker_id?: string | null;
  }>;
  speakers?: Array<{ id: string; total_speech_s: number }>;
}): Transcript {
  return {
    asset_stem: opts.stem,
    language: "en",
    diarized: opts.diarized ?? false,
    segments: (opts.segments ?? []).map((s) => ({
      text: s.text,
      start_s: s.start_s,
      end_s: s.end_s,
      speaker_id: s.speaker_id ?? null,
    })),
    words: [],
    speakers: opts.speakers ?? [],
  };
}

function loadTranscript(t: Transcript, makeActive = true): void {
  useTranscriptStore.setState((s) => ({
    byStem: { ...s.byStem, [t.asset_stem]: { state: "loaded", transcript: t } },
    activeStem: makeActive ? t.asset_stem : s.activeStem,
  }));
}

// 1. No snapshot, no transcript → everything empty + availability all false
{
  resetStores();
  assert.deepEqual(selectScenes(), []);
  assert.deepEqual(selectSpeakers(), []);
  assert.deepEqual(selectSilences(), []);
  assert.deepEqual(selectTranscriptSegments(), []);
  assert.deepEqual(selectCaptions(), []);
  assert.deepEqual(selectColorStats(), []);
  assert.deepEqual(selectMotionRegions(), []);
  assert.deepEqual(selectFaces(), []);
  assert.deepEqual(selectAudioSamples(), []);
  const avail = selectEvidenceAvailability();
  assert.equal(avail.transcript, false);
  assert.equal(avail.speakers, false);
  assert.equal(avail.captions, false);
  assert.equal(avail.scenes, false);
  assert.equal(avail.silences, false);
  assert.equal(avail.color, false);
  assert.equal(avail.motion, false);
  assert.equal(avail.faces, false);
  assert.equal(avail.audio, false);
  assert.equal(avail.episodes, false);
  assert.deepEqual(selectEpisodes(), []);
}

// 2. activeTranscript() returns null when no stem is active
{
  resetStores();
  assert.equal(activeTranscript(), null);
}

// 3. Transcript loaded but not active → still null. Accessors stay []
{
  resetStores();
  loadTranscript(
    transcript({
      stem: "asset-a",
      segments: [{ text: "hello", start_s: 0, end_s: 1 }],
    }),
    /* makeActive */ false,
  );
  assert.equal(activeTranscript(), null);
  assert.deepEqual(selectTranscriptSegments(), []);
}

// 4. Snapshot says transcripts:true but no Transcript loaded → availability
//    still reports transcript:false. We require BOTH signals so a stale
//    "ready" flag doesn't claim data we can't actually render.
{
  resetStores();
  useIndexReadinessStore.setState({ snapshot: snapshot({ transcripts: true }) });
  const avail = selectEvidenceAvailability();
  assert.equal(avail.transcript, false);
  assert.equal(avail.captions, false);
  assert.equal(avail.speakers, false);
}

// 5. Loaded transcript + matching snapshot → segments + captions surface
{
  resetStores();
  useIndexReadinessStore.setState({ snapshot: snapshot() });
  loadTranscript(
    transcript({
      stem: "asset-b",
      segments: [
        { text: "Hello world", start_s: 0, end_s: 1.5, speaker_id: "SPEAKER_00" },
        { text: "How are you", start_s: 1.5, end_s: 2.8, speaker_id: "SPEAKER_01" },
      ],
    }),
  );
  const segs = selectTranscriptSegments();
  assert.equal(segs.length, 2);
  assert.equal(segs[0]!.id, "asset-b-seg-0");
  assert.equal(segs[0]!.text, "Hello world");
  assert.equal(segs[0]!.speaker_id, "SPEAKER_00");
  assert.equal(segs[0]!.start_s, 0);
  assert.equal(segs[0]!.end_s, 1.5);
  assert.equal(segs[1]!.id, "asset-b-seg-1");
  assert.equal(segs[1]!.speaker_id, "SPEAKER_01");

  const caps = selectCaptions();
  assert.equal(caps.length, 2);
  assert.equal(caps[0]!.id, "asset-b-cap-0");
  assert.equal(caps[0]!.text, "Hello world");
  assert.equal(caps[0]!.start_s, 0);
  assert.equal(caps[0]!.end_s, 1.5);

  const avail = selectEvidenceAvailability();
  assert.equal(avail.transcript, true);
  assert.equal(avail.captions, true);
}

// 6. Segment with null speaker_id → projected to undefined (not null)
{
  resetStores();
  useIndexReadinessStore.setState({ snapshot: snapshot() });
  loadTranscript(
    transcript({
      stem: "asset-c",
      segments: [{ text: "no diarization here", start_s: 0, end_s: 2 }],
    }),
  );
  const segs = selectTranscriptSegments();
  assert.equal(segs.length, 1);
  assert.equal(segs[0]!.speaker_id, undefined);
}

// 7. Non-diarized transcript → useSpeakers returns [], availability.speakers false
{
  resetStores();
  useIndexReadinessStore.setState({ snapshot: snapshot({ speaker: false }) });
  loadTranscript(
    transcript({
      stem: "asset-d",
      diarized: false,
      segments: [{ text: "mono speaker", start_s: 0, end_s: 1 }],
    }),
  );
  assert.deepEqual(selectSpeakers(), []);
  assert.equal(selectEvidenceAvailability().speakers, false);
}

// 8. Diarized transcript → useSpeakers returns speakers with segment counts
{
  resetStores();
  useIndexReadinessStore.setState({ snapshot: snapshot() });
  loadTranscript(
    transcript({
      stem: "asset-e",
      diarized: true,
      segments: [
        { text: "a", start_s: 0, end_s: 1, speaker_id: "SPEAKER_00" },
        { text: "b", start_s: 1, end_s: 2, speaker_id: "SPEAKER_01" },
        { text: "c", start_s: 2, end_s: 3, speaker_id: "SPEAKER_00" },
        { text: "d", start_s: 3, end_s: 4, speaker_id: "SPEAKER_00" },
      ],
      speakers: [
        { id: "SPEAKER_00", total_speech_s: 3.0 },
        { id: "SPEAKER_01", total_speech_s: 1.0 },
      ],
    }),
  );
  const speakers = selectSpeakers();
  assert.equal(speakers.length, 2);
  const s00 = speakers.find((s) => s.id === "SPEAKER_00");
  const s01 = speakers.find((s) => s.id === "SPEAKER_01");
  assert.equal(s00!.label, "SPEAKER_00");
  assert.equal(s00!.segment_count, 3);
  assert.equal(s00!.total_speech_s, 3.0);
  assert.equal(s01!.segment_count, 1);
  assert.equal(s01!.total_speech_s, 1.0);
  assert.equal(s00!.portrait_path, undefined);

  const avail = selectEvidenceAvailability();
  assert.equal(avail.speakers, true);
}

// 9. Diarized snapshot but speakers[] empty → availability.speakers false.
//    Defensive: a diarized=true flag with no rows is broken data; treat
//    as not-reachable so the chip stays greyed-out instead of clickable
//    and empty.
{
  resetStores();
  useIndexReadinessStore.setState({ snapshot: snapshot() });
  loadTranscript(
    transcript({
      stem: "asset-f",
      diarized: true,
      segments: [{ text: "x", start_s: 0, end_s: 1, speaker_id: "SPEAKER_00" }],
      speakers: [],
    }),
  );
  assert.equal(selectEvidenceAvailability().speakers, false);
}

// 10. Snapshot reports gaps as false-on-disk too → availability reflects it
//     for transcript/speakers/captions; the other six stay false regardless
//     because the data isn't reachable from the frontend.
{
  resetStores();
  useIndexReadinessStore.setState({
    snapshot: snapshot({
      transcripts: false,
      speaker: false,
      captions: false,
      scenes: false,
      audio: false,
      face: false,
      motion: false,
      color: false,
      silence: false,
    }),
  });
  loadTranscript(
    transcript({
      stem: "asset-g",
      segments: [{ text: "hi", start_s: 0, end_s: 1 }],
    }),
  );
  const avail = selectEvidenceAvailability();
  assert.equal(avail.transcript, false);
  assert.equal(avail.captions, false);
  assert.equal(avail.speakers, false);
}

// 11. Wave 3 C1 — all indexers ready on disk, but the lazy-load cache
//     has nothing fetched yet. The six lazy accessors now report
//     `available: true` based on the on-disk readiness flag (fallback),
//     so the chip surfaces as clickable and the drill-down owns the
//     "loading…" visual. The selectors still return [] until a fetch
//     populates the cache.
{
  resetStores();
  useIndexReadinessStore.setState({ snapshot: snapshot() });
  const avail = selectEvidenceAvailability();
  // Fallback to snapshot flags when nothing is cached yet.
  assert.equal(avail.scenes, true);
  assert.equal(avail.silences, true);
  assert.equal(avail.color, true);
  assert.equal(avail.motion, true);
  assert.equal(avail.faces, true);
  assert.equal(avail.audio, true);
  // Selectors still return [] until the lazy fetch resolves.
  assert.deepEqual(selectScenes(), []);
  assert.deepEqual(selectSilences(), []);
  assert.deepEqual(selectColorStats(), []);
  assert.deepEqual(selectMotionRegions(), []);
  assert.deepEqual(selectFaces(), []);
  assert.deepEqual(selectAudioSamples(), []);
}

// 12. Pure helper evidenceAvailabilityFromState — undefined snapshot is
//     all-false regardless of any loaded transcript. This guards against
//     a project-load race where transcripts are cached but the readiness
//     load hasn't completed.
{
  const avail = evidenceAvailabilityFromState({
    snapshot: undefined,
    loadedTranscript: transcript({
      stem: "x",
      segments: [{ text: "a", start_s: 0, end_s: 1 }],
    }),
  });
  assert.equal(avail.transcript, false);
  assert.equal(avail.speakers, false);
  assert.equal(avail.captions, false);
  assert.equal(avail.scenes, false);
}

// 13. Switching active stem to one with no loaded transcript drops
//     segments back to [].
{
  resetStores();
  useIndexReadinessStore.setState({ snapshot: snapshot() });
  loadTranscript(
    transcript({
      stem: "asset-h",
      segments: [{ text: "first", start_s: 0, end_s: 1 }],
    }),
  );
  assert.equal(selectTranscriptSegments().length, 1);
  useTranscriptStore.setState({ activeStem: "asset-not-loaded" });
  assert.deepEqual(selectTranscriptSegments(), []);
  assert.equal(selectEvidenceAvailability().transcript, false);
}

// 14. Empty transcript (segments=[], speakers=[]) is still "loaded".
//     The accessor returns []; availability.transcript is true because
//     transcripts:true on the snapshot AND we have a Transcript object
//     even if it's empty.
{
  resetStores();
  useIndexReadinessStore.setState({ snapshot: snapshot() });
  loadTranscript(transcript({ stem: "asset-i" }));
  assert.deepEqual(selectTranscriptSegments(), []);
  assert.deepEqual(selectCaptions(), []);
  assert.equal(selectEvidenceAvailability().transcript, true);
  assert.equal(selectEvidenceAvailability().captions, true);
}

// 15. Wave 3 C1 — when the lazy-load cache is seeded with scenes for the
//     active (project, stem) pair, selectScenes returns those rows and
//     evidence availability reflects "has data". Mirrors the Tauri
//     fetch path without invoking it.
{
  resetStores();
  useIndexReadinessStore.setState({ snapshot: snapshot() });
  useProjectStore.setState({ current: "/tmp/project-x" });
  loadTranscript(
    transcript({
      stem: "asset-x",
      segments: [{ text: "hi", start_s: 0, end_s: 1 }],
    }),
  );
  const scenes: EvidenceScene[] = [
    { id: "scene-0", start_s: 0, end_s: 1.5 },
    { id: "scene-1", start_s: 1.5, end_s: 4.0 },
  ];
  __setEvidenceDataForTests("/tmp/project-x", "asset-x", "scenes", scenes);
  assert.equal(selectScenes().length, 2);
  assert.equal(selectScenes()[0]!.id, "scene-0");
  const avail = selectEvidenceAvailability();
  assert.equal(avail.scenes, true);
}

// 16. Switching project wipes the lazy-load cache (scope guard).
{
  resetStores();
  useIndexReadinessStore.setState({ snapshot: snapshot() });
  useProjectStore.setState({ current: "/tmp/project-a" });
  loadTranscript(
    transcript({
      stem: "asset-a",
      segments: [{ text: "hi", start_s: 0, end_s: 1 }],
    }),
  );
  __setEvidenceDataForTests("/tmp/project-a", "asset-a", "silences", [
    { start_s: 0, end_s: 1, duration_s: 1 },
  ]);
  assert.equal(selectSilences().length, 1);
  // Switching the project should drop the project-a cache when the
  // next selector reads (it eagerly re-scopes on cache touch).
  useProjectStore.setState({ current: "/tmp/project-b" });
  // Reading silences with no seeded data for project-b returns []
  // and triggers no invoke (we're not in Tauri).
  assert.deepEqual(selectSilences(), []);
}

// 17. Wave 6 B1 — selectEpisodes returns rows from the episodes store
//     and orders them by `order`, regardless of insertion order. The
//     availability map flips `episodes: true` when the store has at
//     least one row, independently of the indexer readiness snapshot.
{
  resetStores();
  useEpisodesStore.setState({
    snapshot: episodes([
      {
        id: "ep-c",
        name: "Third",
        order: 3,
        startS: 200,
        endS: 300,
        durationS: 100,
        confidence: 0.6,
        status: "rejected",
        evidenceCount: 1,
      },
      {
        id: "ep-a",
        name: "First",
        order: 1,
        startS: 0,
        endS: 60,
        durationS: 60,
        confidence: 0.92,
        status: "accepted",
        evidenceCount: 12,
      },
      {
        id: "ep-b",
        name: "Second",
        order: 2,
        startS: 60,
        endS: 200,
        durationS: 140,
        confidence: 0.7,
        status: "review_needed",
        evidenceCount: 4,
      },
    ]),
  });
  const eps = selectEpisodes();
  assert.equal(eps.length, 3);
  // Sorted by order ascending.
  assert.equal(eps[0]!.id, "ep-a");
  assert.equal(eps[1]!.id, "ep-b");
  assert.equal(eps[2]!.id, "ep-c");
  assert.equal(eps[0]!.name, "First");
  assert.equal(eps[0]!.evidenceCount, 12);
  assert.equal(eps[0]!.status, "accepted");
  // Episodes flip availability regardless of readiness snapshot state.
  assert.equal(selectEvidenceAvailability().episodes, true);
}

// 18. Empty episodes store → availability.episodes false, selector []
{
  resetStores();
  useEpisodesStore.setState({ snapshot: episodes([]) });
  assert.deepEqual(selectEpisodes(), []);
  assert.equal(selectEvidenceAvailability().episodes, false);
}

// 19. Undefined episodes snapshot is treated the same as no rows.
{
  resetStores();
  useEpisodesStore.setState({ snapshot: undefined });
  assert.deepEqual(selectEpisodes(), []);
  assert.equal(selectEvidenceAvailability().episodes, false);
}

// 20. Episodes surface even when no readiness snapshot is loaded yet
//     (e.g. project just opened, indexer call in-flight). They live in
//     OTIO metadata, not on disk, so the readiness signal doesn't gate
//     them.
{
  const avail = evidenceAvailabilityFromState({
    snapshot: undefined,
    loadedTranscript: null,
    episodeCount: 2,
  });
  assert.equal(avail.episodes, true);
  // Other flags stay false — undefined snapshot still means "nothing
  // reachable" for the indexer-backed kinds.
  assert.equal(avail.scenes, false);
  assert.equal(avail.transcript, false);
}

// 21. Episodes with missing/empty name fall back to "Episode N" in
//     the panel, but the selector preserves the raw value so the UI
//     owns the fallback (no double-formatting).
{
  resetStores();
  useEpisodesStore.setState({
    snapshot: episodes([
      {
        id: "ep-x",
        name: "",
        order: 5,
        startS: 0,
        endS: 30,
        durationS: 30,
        confidence: 1,
        status: "accepted",
        evidenceCount: 0,
      },
    ]),
  });
  const eps = selectEpisodes();
  assert.equal(eps[0]!.name, "");
  assert.equal(eps[0]!.order, 5);
  assert.equal(eps[0]!.evidenceCount, 0);
}

console.log("evidence-accessors: OK");
