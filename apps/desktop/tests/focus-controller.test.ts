// Tests for useFocusController — the Wave 4 W4.6 Review → dispatcher.
//
// What we lock in here:
//   1. deriveRanges turns diff_hints into (track, start, end) ranges,
//      picking the right snapshot per hint kind.
//   2. unionRange covers the full span when multiple ranges arrive.
//   3. focusProposal routes mediums to the right CenterMode and pushes
//      seek + flash + sub-tab + toast through the adapter / sub-stores.
//   4. flashes auto-clear after the configured timeout.
//
// We swap the adapter to a synchronous double per test; the sub-stores
// (`useFlashRanges`, `useSourceFocus`, `useTranscriptFlashes`) are the
// real Zustand stores — exercising them through the public API mirrors
// the production wire.

import { strict as assert } from "node:assert";
import type { AppliedDiff, TimelineSnapshot } from "../src/protocol";
import type { CenterMode } from "../src/state/centerMode.ts";
import {
  deriveRanges,
  unionRange,
  useFlashRanges,
  useFocusController,
  useSourceFocus,
  useTranscriptFlashes,
  type FocusAdapter,
} from "../src/state/focusController.ts";

function makeSnapshot(
  tracks: Array<Array<{ index: number; start: number; duration: number }>>,
): TimelineSnapshot {
  return {
    duration_s: 60,
    broadcast_overlay: null,
    cut_boundaries: [],
    preview_limitations: [],
    tracks: tracks.map((items, ti) => ({
      name: `V${ti + 1}`,
      kind: "video",
      role: null,
      items: items.map((it) => ({
        index: it.index,
        kind: "clip",
        track_start_s: it.start,
        duration_s: it.duration,
        source_path: null,
        source_start_s: 0,
        proxy_path: null,
        thumbnail_path: null,
        waveform_path: null,
        color_correction: null,
        title: null,
        video_overlay: null,
        motion_shape: null,
        motion_image: null,
        broadcast_overlay: null,
        audio: null,
        animations: [],
      } as unknown as TimelineSnapshot["tracks"][number]["items"][number])),
    })) as unknown as TimelineSnapshot["tracks"],
  };
}

interface Recorder {
  centerMode: CenterMode[];
  seeks: number[];
  scrolls: number[];
  snapshot: TimelineSnapshot | null;
}

function installRecorder(snapshot: TimelineSnapshot | null = null): Recorder {
  const rec: Recorder = {
    centerMode: [],
    seeks: [],
    scrolls: [],
    snapshot,
  };
  const adapter: FocusAdapter = {
    setCenterMode: (m) => rec.centerMode.push(m),
    requestTimelineSeek: (t) => rec.seeks.push(t),
    scrollTimelineTo: (t) => rec.scrolls.push(t),
    readTimelineSnapshot: () => rec.snapshot,
  };
  useFocusController.setState({ adapter });
  useFlashRanges.getState().clear();
  useSourceFocus.getState().clear();
  useTranscriptFlashes.getState().clear();
  return rec;
}

// unionRange returns null on empty input, the span on multi-range input
{
  assert.equal(unionRange([]), null);
  const u = unionRange([
    { trackIndex: 0, startS: 5, endS: 7 },
    { trackIndex: 1, startS: 2, endS: 4 },
    { trackIndex: 0, startS: 8, endS: 9 },
  ]);
  assert.deepEqual(u, { startS: 2, endS: 9 });
}

// deriveRanges — delete hint reads the current snapshot
{
  const current = makeSnapshot([
    [
      { index: 0, start: 0, duration: 2 },
      { index: 1, start: 2, duration: 3 },
    ],
  ]);
  const proposed = makeSnapshot([[{ index: 0, start: 0, duration: 2 }]]);
  const hints: AppliedDiff[] = [
    { kind: "delete", op_index: 0, track_index: 0, item_index: 1 },
  ];
  const ranges = deriveRanges(hints, current, proposed);
  assert.equal(ranges.length, 1);
  assert.equal(ranges[0].trackIndex, 0);
  assert.equal(ranges[0].startS, 2);
  assert.equal(ranges[0].endS, 5);
}

// deriveRanges — trim_edge / insert / move read the proposed snapshot
{
  const current = makeSnapshot([[{ index: 0, start: 0, duration: 10 }]]);
  const proposed = makeSnapshot([
    [
      { index: 0, start: 0, duration: 5 },
      { index: 1, start: 5, duration: 4 },
    ],
  ]);
  const hints: AppliedDiff[] = [
    { kind: "insert", op_index: 0, track_index: 0, item_index: 1 },
    { kind: "trim_edge", op_index: 1, track_index: 0, item_index: 0, side: "end", delta_s: -5 },
  ];
  const ranges = deriveRanges(hints, current, proposed);
  assert.equal(ranges.length, 2);
  // insert hit
  assert.equal(ranges[0].startS, 5);
  assert.equal(ranges[0].endS, 9);
  // trim_edge hit
  assert.equal(ranges[1].startS, 0);
  assert.equal(ranges[1].endS, 5);
}

// deriveRanges — silently skips hints that don't resolve
{
  const ranges = deriveRanges(
    [{ kind: "delete", op_index: 0, track_index: 99, item_index: 5 }],
    makeSnapshot([[{ index: 0, start: 0, duration: 2 }]]),
    null,
  );
  assert.equal(ranges.length, 0);
}

// cut medium → timeline tab + seek to midpoint + scroll + flash
{
  const current = makeSnapshot([
    [{ index: 0, start: 0, duration: 2 }, { index: 1, start: 2, duration: 4 }],
  ]);
  const rec = installRecorder(current);
  useFocusController.getState().focusProposal({
    proposalId: "p1",
    medium: "cut",
    diffHints: [{ kind: "delete", op_index: 0, track_index: 0, item_index: 1 }],
    proposedSnapshot: null,
  });
  assert.deepEqual(rec.centerMode, ["timeline"]);
  // delete range is [2, 6] — midpoint 4.
  assert.deepEqual(rec.seeks, [4]);
  assert.deepEqual(rec.scrolls, [4]);
  const ranges = useFlashRanges.getState().ranges;
  assert.equal(ranges.length, 1);
  assert.equal(ranges[0].trackIndex, 0);
  assert.equal(ranges[0].startS, 2);
  assert.equal(ranges[0].endS, 6);
  assert.equal(ranges[0].kind, "clip");
}

// transition medium emits a transition-kind flash
{
  const current = makeSnapshot([[{ index: 0, start: 0, duration: 2 }]]);
  const rec = installRecorder(current);
  useFocusController.getState().focusProposal({
    proposalId: "p2",
    medium: "transition",
    diffHints: [{ kind: "delete", op_index: 0, track_index: 0, item_index: 0 }],
    proposedSnapshot: null,
  });
  assert.deepEqual(rec.centerMode, ["timeline"]);
  const r = useFlashRanges.getState().ranges;
  assert.equal(r.length, 1);
  assert.equal(r[0].kind, "transition");
}

// color medium → source + Video sub-tab + before/after toast
{
  const current = makeSnapshot([[{ index: 0, start: 4, duration: 6 }]]);
  const rec = installRecorder(current);
  useFocusController.getState().focusProposal({
    proposalId: "p3",
    medium: "color",
    diffHints: [{ kind: "delete", op_index: 0, track_index: 0, item_index: 0 }],
    proposedSnapshot: null,
  });
  assert.deepEqual(rec.centerMode, ["source"]);
  assert.equal(useSourceFocus.getState().subTab, "video");
  assert.equal(useSourceFocus.getState().toast, "before-after");
  // color seeks to range start (4), not midpoint.
  assert.deepEqual(rec.seeks, [4]);
  // no flash on color — preview-side surfaces don't have a canvas pass.
  assert.equal(useFlashRanges.getState().ranges.length, 0);
}

// broll medium → source + Video sub-tab + insert-preview toast
{
  const current = makeSnapshot([[{ index: 0, start: 3, duration: 5 }]]);
  installRecorder(current);
  useFocusController.getState().focusProposal({
    proposalId: "p4",
    medium: "broll",
    diffHints: [{ kind: "delete", op_index: 0, track_index: 0, item_index: 0 }],
    proposedSnapshot: null,
  });
  assert.equal(useSourceFocus.getState().subTab, "video");
  assert.equal(useSourceFocus.getState().toast, "insert-preview");
}

// title medium → source + transcript flash registered
{
  const current = makeSnapshot([[{ index: 0, start: 12, duration: 4 }]]);
  installRecorder(current);
  useFocusController.getState().focusProposal({
    proposalId: "p5",
    medium: "title",
    diffHints: [{ kind: "delete", op_index: 0, track_index: 0, item_index: 0 }],
    proposedSnapshot: null,
  });
  const flashes = useTranscriptFlashes.getState().flashes;
  assert.equal(flashes.length, 1);
  assert.equal(flashes[0].key, "p5");
  assert.equal(flashes[0].startS, 12);
  assert.equal(flashes[0].endS, 16);
}

// missing diff_hints still switches the tab (no flash, no seek)
{
  installRecorder(null);
  useFocusController.getState().focusProposal({
    proposalId: "p6",
    medium: "cut",
    diffHints: [],
  });
  assert.deepEqual(useFocusController.getState().adapter.readTimelineSnapshot(), null);
  assert.equal(useFlashRanges.getState().ranges.length, 0);
}

// flashes auto-clear after ~600ms
{
  installRecorder(null);
  useFlashRanges.getState().add(
    {
      key: "f1",
      trackIndex: 0,
      startS: 0,
      endS: 1,
      kind: "clip",
    },
    20,
  );
  assert.equal(useFlashRanges.getState().ranges.length, 1);
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.equal(useFlashRanges.getState().ranges.length, 0);
}

// setSubTab bumps subTabRequestId every call (even repeats), so the
// TranscriptSource effect re-fires on stay-on-video clicks.
{
  installRecorder(null);
  const beforeId = useSourceFocus.getState().subTabRequestId;
  useSourceFocus.getState().setSubTab("video");
  const afterFirst = useSourceFocus.getState().subTabRequestId;
  useSourceFocus.getState().setSubTab("video");
  const afterSecond = useSourceFocus.getState().subTabRequestId;
  assert.ok(afterFirst > beforeId, "first set bumps id");
  assert.ok(afterSecond > afterFirst, "second set still bumps id");
}

console.log("focus-controller: OK");
