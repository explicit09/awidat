// Tests for the pure logic behind <TranscriptSource> (Wave 4 W4.4):
//
//   - `buildDeleteRangeOpsForStem` — word-range → Split+Split+Delete
//     envelope. Mirrors the v1 contract from TranscriptView's private
//     helper: only the fully-inside-one-clip case emits ops; multi-
//     clip and empty ranges return [].
//
//   - `isRangeCutFromTimeline` — given a source-time range and the
//     timeline snapshot, decide whether the range is "gone from the
//     cut". Used by AppliedCutsOverlay to dim transcript spans that
//     accepted cuts have removed.
//
//   - `isTranscriptFirstProjectType` — routing predicate the App
//     branches on. Podcast / tutorial route to the new surface;
//     shorts / other / null stay on the legacy preview.

import { strict as assert } from "node:assert";
import {
  buildDeleteRangeOpsForStem,
  isRangeCutFromTimeline,
  isTranscriptFirstProjectType,
} from "../src/shell/source/transcriptSourceLogic.ts";
import type { TimelineSnapshot } from "../src/protocol";

function makeClipItem(opts: {
  clipUuid: string;
  proxyPath: string | null;
  sourceStartS: number;
  durationS: number;
}) {
  return {
    kind: "clip" as const,
    clip_uuid: opts.clipUuid,
    name: opts.clipUuid,
    enabled: true,
    duration_s: opts.durationS,
    media_reference: null,
    timeline_start_s: 0,
    source_start_s: opts.sourceStartS,
    source_end_s: opts.sourceStartS + opts.durationS,
    proxy_path: opts.proxyPath,
    playable_kind: { kind: "available" } as unknown,
    source_path: null,
    color_correction: null,
    audio_fade_in_s: null,
    audio_fade_out_s: null,
    audio_lead_s: null,
    audio_trail_s: null,
    audio_lead_reason: null,
    audio_trail_reason: null,
    audio_lead_confidence: null,
    audio_trail_confidence: null,
    speed_factor: null,
    volume: null,
    motion_image: null,
    motion_shape: null,
    title: null,
    transition_id: null,
    transition_family: null,
    sync_group_id: null,
    sync_offset_s: null,
    sync_speed_factor: null,
    sync_confidence: null,
    cut_intent: null,
    cut_intent_audio: null,
    cut_intent_energy: null,
    cut_intent_confidence: null,
    cut_intent_reason: null,
    animations: [],
  };
}

function snapshotWithClips(
  clips: Array<ReturnType<typeof makeClipItem>>,
): TimelineSnapshot {
  return {
    duration_s: 0,
    broadcast_overlay: null,
    cut_boundaries: [],
    preview_limitations: [],
    tracks: [
      {
        name: "V1",
        kind: "video",
        role: null,
        audio: null,
        // The cast lets us reuse the production type without exhaustively
        // typing every optional field on TimelineItem in node-only test
        // land. `buildDeleteRangeOpsForStem` only reads the fields the
        // fixture sets.
        items: clips as unknown as TimelineSnapshot["tracks"][number]["items"],
      },
    ],
  };
}

/* ------------------- buildDeleteRangeOpsForStem -------------------- */

// Empty / inverted range → no ops.
{
  const snap = snapshotWithClips([
    makeClipItem({
      clipUuid: "clip-a",
      proxyPath: "/proj/.awidat/proxies/foo.mp4",
      sourceStartS: 0,
      durationS: 60,
    }),
  ]);
  assert.deepEqual(
    buildDeleteRangeOpsForStem({
      snapshot: snap,
      stem: "foo",
      sourceStart: 10,
      sourceEnd: 10,
    }),
    [],
    "zero-duration range returns []",
  );
  assert.deepEqual(
    buildDeleteRangeOpsForStem({
      snapshot: snap,
      stem: "foo",
      sourceStart: 20,
      sourceEnd: 15,
    }),
    [],
    "inverted range returns []",
  );
}

// Stem with no matching clip → no ops.
{
  const snap = snapshotWithClips([
    makeClipItem({
      clipUuid: "clip-a",
      proxyPath: "/proj/.awidat/proxies/foo.mp4",
      sourceStartS: 0,
      durationS: 60,
    }),
  ]);
  assert.deepEqual(
    buildDeleteRangeOpsForStem({
      snapshot: snap,
      stem: "bar", // different stem
      sourceStart: 5,
      sourceEnd: 10,
    }),
    [],
  );
}

// Multi-clip span (range crosses a clip boundary) → no ops.
{
  const snap = snapshotWithClips([
    makeClipItem({
      clipUuid: "clip-a",
      proxyPath: "/proj/.awidat/proxies/foo.mp4",
      sourceStartS: 0,
      durationS: 30,
    }),
    makeClipItem({
      clipUuid: "clip-b",
      proxyPath: "/proj/.awidat/proxies/foo.mp4",
      sourceStartS: 30,
      durationS: 30,
    }),
  ]);
  assert.deepEqual(
    buildDeleteRangeOpsForStem({
      snapshot: snap,
      stem: "foo",
      sourceStart: 25,
      sourceEnd: 40, // straddles 30s boundary
    }),
    [],
    "multi-clip spans return [] in v1",
  );
}

// Fully-inside-one-clip range → Split + Split + Delete.
{
  const snap = snapshotWithClips([
    makeClipItem({
      clipUuid: "clip-a",
      proxyPath: "/proj/.awidat/proxies/foo.mp4",
      sourceStartS: 0,
      durationS: 60,
    }),
  ]);
  const ops = buildDeleteRangeOpsForStem({
    snapshot: snap,
    stem: "foo",
    sourceStart: 10,
    sourceEnd: 20,
  });
  assert.equal(ops.length, 3, "expect 3 ops (split, split, delete)");
  assert.equal(ops[0].kind, "split_clip");
  assert.equal(ops[1].kind, "split_clip");
  assert.equal(ops[2].kind, "delete_clip");
  // First split anchors the parent clip at the range start.
  assert.deepEqual(
    ops[0],
    {
      kind: "split_clip",
      anchor: { kind: "clip_uuid", uuid: "clip-a" },
      atS: 10,
    },
  );
  // Second split anchors the post-split right piece (`<parent>-b`) at
  // the range end.
  assert.deepEqual(
    ops[1],
    {
      kind: "split_clip",
      anchor: { kind: "clip_uuid", uuid: "clip-a-b" },
      atS: 20,
    },
  );
  // Delete drops the middle piece (`<parent>-b` after the second
  // split's left-half names it).
  assert.deepEqual(
    ops[2],
    {
      kind: "delete_clip",
      anchor: { kind: "clip_uuid", uuid: "clip-a-b" },
    },
  );
}

// proxy_path=null clip is skipped.
{
  const snap = snapshotWithClips([
    makeClipItem({
      clipUuid: "clip-a",
      proxyPath: null,
      sourceStartS: 0,
      durationS: 60,
    }),
  ]);
  assert.deepEqual(
    buildDeleteRangeOpsForStem({
      snapshot: snap,
      stem: "foo",
      sourceStart: 5,
      sourceEnd: 10,
    }),
    [],
    "clips without proxy_path are ignored",
  );
}

/* ------------------- isRangeCutFromTimeline ------------------------ */

// Stem with no clips → false (pre-timeline / source-preview case).
{
  const snap = snapshotWithClips([]);
  assert.equal(
    isRangeCutFromTimeline(snap, "foo", 10, 12),
    false,
    "empty timeline never paints the transcript as cut",
  );
}

// Range inside a clip → false (still in the cut).
{
  const snap = snapshotWithClips([
    makeClipItem({
      clipUuid: "clip-a",
      proxyPath: "/proj/.awidat/proxies/foo.mp4",
      sourceStartS: 0,
      durationS: 60,
    }),
  ]);
  assert.equal(isRangeCutFromTimeline(snap, "foo", 10, 12), false);
  assert.equal(isRangeCutFromTimeline(snap, "foo", 59, 60), false);
}

// Range in a gap between two clips → true (cut applied).
{
  const snap = snapshotWithClips([
    makeClipItem({
      clipUuid: "clip-a",
      proxyPath: "/proj/.awidat/proxies/foo.mp4",
      sourceStartS: 0,
      durationS: 10,
    }),
    makeClipItem({
      clipUuid: "clip-b",
      proxyPath: "/proj/.awidat/proxies/foo.mp4",
      sourceStartS: 20,
      durationS: 10,
    }),
  ]);
  assert.equal(
    isRangeCutFromTimeline(snap, "foo", 12, 18),
    true,
    "gap range is cut from cut",
  );
}

// Different stem → false (we only care about the asset we're rendering).
{
  const snap = snapshotWithClips([
    makeClipItem({
      clipUuid: "clip-a",
      proxyPath: "/proj/.awidat/proxies/bar.mp4",
      sourceStartS: 0,
      durationS: 30,
    }),
  ]);
  assert.equal(
    isRangeCutFromTimeline(snap, "foo", 10, 12),
    false,
    "other-stem clips don't affect our stem's cut state",
  );
}

/* ------------------- isTranscriptFirstProjectType ------------------ */

assert.equal(isTranscriptFirstProjectType(null), false);
assert.equal(isTranscriptFirstProjectType({ kind: "podcast" }), true);
assert.equal(isTranscriptFirstProjectType({ kind: "tutorial" }), true);
assert.equal(isTranscriptFirstProjectType({ kind: "shorts" }), false);
assert.equal(
  isTranscriptFirstProjectType({ kind: "other", description: "interview" }),
  false,
  "free-text 'other' stays on legacy preview in v1",
);

console.log("transcript-source: OK");
