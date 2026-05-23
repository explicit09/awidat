import assert from "node:assert/strict";
import {
  buildCutBadges,
  buildSplitOffsets,
  formatEditorialLabel,
  formatMarkerSeconds,
} from "./editorialMarkers.ts";
import type { TimelineItem, TimelineSnapshot } from "./store.ts";

function clip(
  clip_uuid: string,
  track_start_s: number,
  duration_s: number,
  extra: Partial<Extract<TimelineItem, { kind: "clip" }>> = {},
): Extract<TimelineItem, { kind: "clip" }> {
  return {
    kind: "clip",
    index: extra.index ?? 0,
    name: extra.name ?? clip_uuid,
    clip_uuid,
    track_start_s,
    duration_s,
    asset_id: null,
    source_start_s: null,
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
    has_video: null,
    has_audio: null,
    color_correction: null,
    lut_path: null,
    title: null,
    video_overlay: null,
    animations: [],
    ...extra,
  };
}

function snapshot(items: TimelineItem[]): TimelineSnapshot {
  return {
    duration_s: 20,
    broadcast_overlay: null,
    preview_limitations: [],
    cut_boundaries: [],
    tracks: [{ name: "V1", kind: "video", role: null, audio: null, items }],
  };
}

console.log("# formatEditorialLabel");
assert.equal(formatEditorialLabel("shot_reverse_shot"), "Shot Reverse Shot");
console.log("  ok  formats snake-case labels for badges");

console.log("# formatMarkerSeconds");
assert.equal(formatMarkerSeconds(1.5), "1.5s");
assert.equal(formatMarkerSeconds(2), "2s");
console.log("  ok  trims redundant decimal zeros");

console.log("# buildCutBadges");
const cutSnapshot = snapshot([clip("from", 0, 4, { index: 0 }), clip("to", 4, 3, { index: 1 })]);
cutSnapshot.cut_boundaries = [
  {
    key: "from::to",
    from_clip_id: "from",
    to_clip_id: "to",
    cut_type: "shot_reverse_shot",
    intent: "dialogue exchange",
    energy: null,
    audio_relation: "sync",
    confidence: null,
    reason: "matches eyeline",
  },
];
assert.deepEqual(buildCutBadges(cutSnapshot, 10), [
  {
    key: "cut-from::to",
    x: 30,
    y: 24,
    label: "S/RS",
    title: "Shot Reverse Shot - intent: dialogue exchange - audio: sync - matches eyeline",
  },
]);
console.log("  ok  anchors cut badges to incoming clips with formatted metadata");

console.log("# buildSplitOffsets");
const splitSnapshot = snapshot([
  clip("j", 3, 5, {
    audio_lead_s: 1.25,
    audio_trail_s: 2,
    split_edit_reason: "audio establishes next shot",
    split_edit_confidence: 0.82,
  }),
]);
assert.deepEqual(buildSplitOffsets(splitSnapshot, 10), [
  {
    key: "lead-j",
    x: 34,
    y: 66,
    label: "J +1.25s",
    title: "Audio lead: 1.25s - audio establishes next shot - confidence 82%",
  },
  {
    key: "trail-j",
    x: 30,
    y: 66,
    label: "L +2s",
    title: "Audio trail: 2s - audio establishes next shot - confidence 82%",
  },
]);
console.log("  ok  derives J/L split offset markers from clip metadata");
