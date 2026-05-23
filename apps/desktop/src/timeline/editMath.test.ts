import { strict as assert } from "node:assert";
import {
  snapMoveDeltaS,
  targetPositionForMove,
  type UserMoveDrag,
} from "./editMath.ts";
import type { TimelineItem, TimelineSnapshot, TimelineTrack } from "./store.ts";

function describe(name: string, fn: () => void) {
  console.log(`\n# ${name}`);
  fn();
}

function it(name: string, fn: () => void) {
  try {
    fn();
    console.log(`  ok  ${name}`);
  } catch (err: unknown) {
    console.error(`  FAIL  ${name}`);
    console.error("    " + (err instanceof Error ? err.message : String(err)));
    process.exitCode = 1;
  }
}

function clip(
  index: number,
  uuid: string,
  trackStartS: number,
  durationS: number,
  linkGroupId: string | null = null,
): Extract<TimelineItem, { kind: "clip" }> {
  return {
    kind: "clip",
    index,
    clip_uuid: uuid,
    name: uuid,
    media_path: `${uuid}.mp4`,
    source_start_s: 0,
    source_end_s: durationS,
    duration_s: durationS,
    track_start_s: trackStartS,
    volume: null,
    audio_fade_in_s: null,
    audio_fade_out_s: null,
    audio_lead_s: null,
    audio_trail_s: null,
    link_group_id: linkGroupId,
    sync_group_id: null,
    sync_offset_s: null,
    speed_factor: null,
    color_correction: null,
    lut: null,
    title: null,
    animations: [],
    preview_limitations: [],
    effects: [],
    audio_fx: null,
  };
}

function track(items: TimelineItem[]): TimelineTrack {
  return {
    index: 0,
    name: "V1",
    kind: "Video",
    audio_controls: null,
    items,
  };
}

function snapshot(items: TimelineItem[]): TimelineSnapshot {
  return {
    duration_s: 20,
    broadcast_overlay: null,
    cut_boundaries: [],
    preview_limitations: [],
    tracks: [track(items)],
  };
}

describe("targetPositionForMove", () => {
  it("returns the first item index whose midpoint is after the target start", () => {
    const items = [clip(0, "a", 0, 4), clip(1, "b", 4, 4), clip(2, "c", 8, 4)];

    assert.equal(targetPositionForMove(items, 2, 1), 0);
    assert.equal(targetPositionForMove(items, 0, 6.5), 2);
  });

  it("ignores the moving item and appends after the last midpoint", () => {
    const items = [clip(0, "a", 0, 4), clip(1, "b", 4, 4), clip(2, "c", 8, 4)];

    assert.equal(targetPositionForMove(items, 1, 12), 3);
  });
});

describe("snapMoveDeltaS", () => {
  it("snaps the moved clip start to a nearby playhead", () => {
    const drag: UserMoveDrag = {
      trackIndex: 0,
      clipIndex: 0,
      clipUuid: "a",
      linkGroupId: null,
      startX: 0,
      currentX: 505,
      startY: 0,
      currentY: 0,
    };

    assert.equal(snapMoveDeltaS(snapshot([clip(0, "a", 0, 2)]), 5, drag, 100), 5);
  });

  it("excludes linked clips from snap targets while moving a group", () => {
    const drag: UserMoveDrag = {
      trackIndex: 0,
      clipIndex: 0,
      clipUuid: "a",
      linkGroupId: "sync-1",
      startX: 0,
      currentX: 395,
      startY: 0,
      currentY: 0,
    };
    const shotA = clip(0, "a", 0, 2, "sync-1");
    const linkedAudio = clip(1, "a-audio", 4, 2, "sync-1");
    const unrelated = clip(2, "b", 8, 2);

    assert.equal(snapMoveDeltaS(snapshot([shotA, linkedAudio, unrelated]), 20, drag, 100), 3.95);
  });
});
