import { strict as assert } from "node:assert";
import type { AppliedDiff, TimelineSnapshot } from "../src/protocol";
import {
  deriveRanges,
  unionRange,
  useFlashRanges,
  useFocusController,
  type FocusAdapter,
} from "../src/state/focusController.ts";

function makeSnapshot(
  tracks: Array<Array<{ index: number; start: number; duration: number; uuid?: string }>>,
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
        clip_uuid: it.uuid ?? `clip-${ti}-${it.index}`,
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
  seeks: number[];
  scrolls: number[];
  snapshot: TimelineSnapshot | null;
}

function installRecorder(snapshot: TimelineSnapshot | null = null): Recorder {
  const rec: Recorder = {
    seeks: [],
    scrolls: [],
    snapshot,
  };
  const adapter: FocusAdapter = {
    requestTimelineSeek: (t) => rec.seeks.push(t),
    scrollTimelineTo: (t) => rec.scrolls.push(t),
    readTimelineSnapshot: () => rec.snapshot,
  };
  useFocusController.setState({ adapter });
  useFlashRanges.getState().clear();
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

// New insertions cannot be played yet; trims resolve to the current clip.
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
    { kind: "trim_edge", op_index: 1, track_index: 0, item_index: 0, side: "right", delta_s: -5 },
  ];
  const ranges = deriveRanges(hints, current, proposed);
  assert.deepEqual(ranges, [{ trackIndex: 0, startS: 0, endS: 10, reviewTimeS: 5 }]);
}

// Ripple edits change indices and times; trim/split navigation follows identity.
for (const kind of ["trim_edge", "split"] as const) {
  const current = makeSnapshot([[
    { index: 0, start: 0, duration: 10, uuid: "A" },
    { index: 1, start: 10, duration: 10, uuid: "B" },
  ]]);
  const proposed = makeSnapshot([[{ index: 0, start: 0, duration: 5, uuid: "B" }]]);
  const hint: AppliedDiff = kind === "split"
    ? { kind, op_index: 1, track_index: 0, item_index: 0, at_s: 5 }
    : { kind, op_index: 1, track_index: 0, item_index: 0, side: "right", delta_s: -5 };
  assert.deepEqual(deriveRanges([hint], current, proposed),
    [{ trackIndex: 0, startS: 10, endS: 20, reviewTimeS: 15 }]);
}

// Split points and trim edges map source time to playback time, including speed.
for (const speed of [1, 2]) {
  const current = makeSnapshot([[{ index: 0, start: 30, duration: 20, uuid: "A" }]]);
  const proposed = makeSnapshot([[{ index: 0, start: 30, duration: 8 / speed, uuid: "A" }]]);
  Object.assign(current.tracks[0].items[0], { source_start_s: 10, speed });
  Object.assign(proposed.tracks[0].items[0], { source_start_s: 10, speed });
  for (const hint of [
    { kind: "split", op_index: 0, track_index: 0, item_index: 0, at_s: 18 },
    { kind: "trim_edge", op_index: 0, track_index: 0, item_index: 0, side: "right", delta_s: 12 },
  ] as AppliedDiff[]) {
    const ranges = deriveRanges([hint], current, proposed);
    assert.equal(ranges[0].reviewTimeS, 30 + 8 / speed);
    const rec = installRecorder(current);
    useFocusController.getState().focusProposal({
      proposalId: "exact", medium: "cut", diffHints: [hint], proposedSnapshot: proposed,
    });
    assert.deepEqual(rec.seeks, [30 + 8 / speed]);
  }
}

// Moving B before A must still navigate to B in the current playback.
{
  const current = makeSnapshot([[
    { index: 0, start: 0, duration: 10 },
    { index: 1, start: 10, duration: 10 },
  ]]);
  const proposed = makeSnapshot([[
    { index: 0, start: 0, duration: 10 },
    { index: 1, start: 10, duration: 10 },
  ]]);
  const hint: AppliedDiff = { kind: "move", op_index: 0,
    from_track_index: 0, from_item_index: 1, to_track_index: 0, to_item_index: 0 };
  assert.deepEqual(deriveRanges([hint], current, proposed),
    [{ trackIndex: 0, startS: 10, endS: 20 }]);
  const rec = installRecorder(current);
  useFocusController.getState().focusProposal({
    proposalId: "move-b", medium: "cut", diffHints: [hint], proposedSnapshot: proposed,
  });
  assert.deepEqual(rec.seeks, [15]);
  assert.deepEqual(rec.scrolls, [15]);
  assert.equal(useFlashRanges.getState().ranges[0].startS, 10);
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

// cut medium → seek to midpoint + scroll + flash
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
  const r = useFlashRanges.getState().ranges;
  assert.equal(r.length, 1);
  assert.equal(r[0].kind, "transition");
}

// Visual proposals seek the actual preview rather than updating retired tabs.
for (const medium of ["color", "broll", "title", "caption"] as const) {
  const rec = installRecorder(makeSnapshot([[{ index: 0, start: 4, duration: 6 }]]));
  useFocusController.getState().focusProposal({
    proposalId: `visual-${medium}`, medium,
    diffHints: [{ kind: "delete", op_index: 0, track_index: 0, item_index: 0 }],
  });
  assert.deepEqual(rec.seeks, [4]);
  assert.equal(useFlashRanges.getState().ranges.length, 0);
}

// Missing diff hints cause no flash or seek.
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

console.log("focus-controller: OK");
