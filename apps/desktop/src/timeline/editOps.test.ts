import { strict as assert } from "node:assert";
import {
  buildDeleteSelectionOps,
  buildMoveDragOps,
  buildTrimDragOps,
} from "./editOps.ts";
import type { UserMoveDrag, UserTrimDrag } from "./editMath.ts";
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
    name: uuid,
    clip_uuid: uuid,
    track_start_s: trackStartS,
    duration_s: durationS,
    asset_id: null,
    source_start_s: 0,
    proxy_path: null,
    thumbnail_dir: null,
    waveform_path: null,
    volume: null,
    speed: null,
    fade_in_s: null,
    fade_out_s: null,
    audio_lead_s: null,
    audio_trail_s: null,
    split_edit_reason: null,
    split_edit_confidence: null,
    link_group_id: linkGroupId,
    has_video: true,
    has_audio: true,
    color_correction: null,
    lut_path: null,
    title: null,
    video_overlay: null,
    animations: [],
  };
}

function transition(index: number, trackStartS: number): Extract<TimelineItem, { kind: "transition" }> {
  return {
    kind: "transition",
    index,
    track_start_s: trackStartS,
    duration_s: 0.5,
    in_offset_s: 0.25,
    out_offset_s: 0.25,
    effect_name: "SMPTE_Dissolve",
    transition_id: null,
    transition_family: null,
    transition_intent: null,
    transition_energy: null,
    transition_direction: null,
    audio_policy: null,
  };
}

function track(index: number, items: TimelineItem[]): TimelineTrack {
  return {
    index,
    name: `T${index}`,
    kind: "Video",
    audio_controls: null,
    items,
  };
}

function snapshot(tracks: TimelineTrack[]): TimelineSnapshot {
  return {
    duration_s: 20,
    broadcast_overlay: null,
    cut_boundaries: [],
    preview_limitations: [],
    tracks,
  };
}

describe("buildDeleteSelectionOps", () => {
  it("deletes every unique clip in the selected linked group", () => {
    const shot = clip(0, "shot", 0, 2, "sync-1");
    const duplicate = clip(1, "shot", 2, 2, "sync-1");
    const audio = clip(2, "audio", 4, 2, "sync-1");
    const snap = snapshot([track(0, [shot, duplicate, audio])]);

    assert.deepEqual(buildDeleteSelectionOps(snap, { trackIndex: 0, clipIndex: 0 }), [
      { kind: "delete_clip", anchor: { kind: "clip_uuid", uuid: "shot" } },
      { kind: "delete_clip", anchor: { kind: "clip_uuid", uuid: "audio" } },
    ]);
  });

  it("deletes a selected transition by anchoring adjacent clips", () => {
    const snap = snapshot([
      track(0, [clip(0, "a", 0, 2), transition(1, 2), clip(2, "b", 2, 2)]),
    ]);

    assert.deepEqual(buildDeleteSelectionOps(snap, { trackIndex: 0, clipIndex: 1 }), [
      {
        kind: "delete_transition",
        from: { kind: "clip_uuid", uuid: "a" },
        to: { kind: "clip_uuid", uuid: "b" },
      },
    ]);
  });
});

describe("buildTrimDragOps", () => {
  it("emits a left-edge trim when the pointer moves enough", () => {
    const drag: UserTrimDrag = {
      hit: { clipUuid: "clip-1", side: "start", sourceStart: 1, sourceEnd: 5 },
      startX: 0,
      currentX: 24,
    };

    assert.deepEqual(buildTrimDragOps(drag, 12), [
      {
        kind: "trim_clip",
        anchor: { kind: "clip_uuid", uuid: "clip-1" },
        start: 3,
        end: undefined,
      },
    ]);
  });

  it("ignores tiny trim drags", () => {
    const drag: UserTrimDrag = {
      hit: { clipUuid: "clip-1", side: "end", sourceStart: 1, sourceEnd: 5 },
      startX: 10,
      currentX: 11,
    };

    assert.deepEqual(buildTrimDragOps(drag, 12), []);
  });
});

describe("buildMoveDragOps", () => {
  it("builds linked move ops and drops duplicate/no-op drafts", () => {
    const drag: UserMoveDrag = {
      trackIndex: 0,
      clipIndex: 0,
      clipUuid: "shot",
      linkGroupId: "sync-1",
      startX: 0,
      currentX: 100,
      startY: 0,
      currentY: 0,
    };
    const snap = snapshot([
      track(0, [
        clip(0, "shot", 0, 2, "sync-1"),
        clip(1, "shot", 3, 2, "sync-1"),
        clip(2, "audio", 6, 2, "sync-1"),
        clip(3, "other", 10, 2),
      ]),
    ]);

    assert.deepEqual(buildMoveDragOps(snap, 20, drag, 100), [
      {
        kind: "move_clip",
        anchor: { kind: "clip_uuid", uuid: "shot" },
        toPosition: 1,
        atS: 1,
      },
      {
        kind: "move_clip",
        anchor: { kind: "clip_uuid", uuid: "audio" },
        toPosition: 3,
        atS: 7,
      },
    ]);
  });
});
