import { strict as assert } from "node:assert";
import {
  hitTestClipBody,
  hitTestEdge,
  type EdgeHit,
} from "./hitDetect.ts";
import { TRACK_HEADER_WIDTH } from "./layout.ts";
import type { TimelineItem, TimelineSnapshot, TimelineTrack } from "./store.ts";

function clip(index: number, trackStartS: number, durationS: number): Extract<TimelineItem, { kind: "clip" }> {
  return {
    kind: "clip",
    index,
    name: `clip-${index}`,
    clip_uuid: `clip-${index}`,
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
    link_group_id: null,
    has_video: true,
    has_audio: true,
    color_correction: null,
    lut_path: null,
    title: null,
    video_overlay: null,
    animations: [],
  };
}

function snapshot(items: TimelineItem[]): TimelineSnapshot {
  const track: TimelineTrack = {
    index: 0,
    name: "V1",
    kind: "video",
    audio_controls: null,
    items,
  };
  return {
    duration_s: 10,
    broadcast_overlay: null,
    cut_boundaries: [],
    preview_limitations: [],
    tracks: [track],
  };
}

const snap = snapshot([clip(0, 0, 5)]);
const laneY = 22 + 12;
const pps = 20;

assert.equal(
  hitTestClipBody(TRACK_HEADER_WIDTH - 2, laneY, snap, pps),
  null,
  "track-label rail should not hit the first clip",
);
assert.deepEqual(
  hitTestClipBody(TRACK_HEADER_WIDTH + 4, laneY, snap, pps),
  { trackIndex: 0, clipIndex: 0 },
  "clip body should begin after the rail",
);
assert.deepEqual(
  hitTestEdge(TRACK_HEADER_WIDTH + 1, laneY, snap, pps) as EdgeHit,
  {
    trackIndex: 0,
    clipIndex: 0,
    clipUuid: "clip-0",
    side: "start",
    sourceStart: 0,
    sourceEnd: 5,
  },
  "edge hit should use the rail-adjusted clip start",
);

console.log("hit-detect rail offset: OK");
