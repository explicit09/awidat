import {
  AUTO_ADVANCE_GAP_LIMIT_S,
  derivePreviewPlan,
  findActiveSegment,
  findNextSegmentAfter,
  nearestTimelineTimeForSource,
  shouldAutoAdvanceTimelineGap,
  sourceTimeForTimelineTime,
  timelineTimeForSource,
} from "../src/timeline/usePlaySegments";
import type { TimelineSnapshot } from "../src/timeline/store";

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const snapshot: TimelineSnapshot = {
  duration_s: 5,
  tracks: [
    {
      kind: "video",
      role: "primary",
      items: [
        {
          kind: "clip",
          index: 0,
          name: "clip-0",
          clip_uuid: "u-0",
          track_start_s: 0,
          duration_s: 5,
          asset_id: "raw/clip.mov",
          source_start_s: 0,
          proxy_path: null,
          playable_path: "/project/raw/clip.mov",
          playable_kind: "source",
          thumbnail_dir: null,
          waveform_path: null,
          volume: 1,
          speed: 1,
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
          lut_strength: null,
          title: null,
          video_overlay: null,
          animations: [],
          transition_id: null,
          effect_name: null,
          in_offset_s: null,
          out_offset_s: null,
        },
      ],
    },
  ],
} as unknown as TimelineSnapshot;

const plan = derivePreviewPlan(snapshot);
assert(plan.segments.length === 1, "source fallback must enter timeline preview");
assert(
  plan.segments[0].proxyPath === "/project/raw/clip.mov",
  "segment must carry the source-fallback path",
);
console.log("ok  play-segments include source-fallback clips");

const gappedSnapshot: TimelineSnapshot = {
  duration_s: 12,
  tracks: [
    {
      kind: "video",
      role: "primary",
      items: [
        {
          kind: "clip",
          index: 0,
          name: "clip-0",
          clip_uuid: "u-0",
          track_start_s: 0,
          duration_s: 4,
          asset_id: "raw/clip.mov",
          source_start_s: 0,
          proxy_path: null,
          playable_path: "/project/raw/clip.mov",
          playable_kind: "source",
          thumbnail_dir: null,
          waveform_path: null,
          volume: 1,
          speed: 1,
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
          lut_strength: null,
          title: null,
          video_overlay: null,
          animations: [],
          transition_id: null,
          effect_name: null,
          in_offset_s: null,
          out_offset_s: null,
        },
        { kind: "gap", track_start_s: 4, duration_s: 3 },
        {
          kind: "clip",
          index: 2,
          name: "clip-1",
          clip_uuid: "u-1",
          track_start_s: 7,
          duration_s: 5,
          asset_id: "raw/clip.mov",
          source_start_s: 20,
          proxy_path: null,
          playable_path: "/project/raw/clip.mov",
          playable_kind: "source",
          thumbnail_dir: null,
          waveform_path: null,
          volume: 1,
          speed: 1,
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
          lut_strength: null,
          title: null,
          video_overlay: null,
          animations: [],
          transition_id: null,
          effect_name: null,
          in_offset_s: null,
          out_offset_s: null,
        },
      ],
    },
  ],
} as unknown as TimelineSnapshot;

const gappedPlan = derivePreviewPlan(gappedSnapshot);
assert(
  findActiveSegment(gappedPlan.segments, 5) === -1,
  "gap time should not belong to a playable segment",
);
assert(
  findNextSegmentAfter(gappedPlan.segments, 5) === 1,
  "gap seek should resolve to the next playable segment",
);
assert(
  findNextSegmentAfter(gappedPlan.segments, 12) === -1,
  "true timeline end should not resolve to another segment",
);
console.log("ok  gap seeks resolve to the next playable segment");

assert(
  shouldAutoAdvanceTimelineGap(7 - AUTO_ADVANCE_GAP_LIMIT_S / 2, 7),
  "sub-frame/micro gaps should auto-advance",
);
assert(
  !shouldAutoAdvanceTimelineGap(5, 7),
  "real timeline gaps should not silently jump to the next segment",
);
console.log("ok  preview only auto-advances across micro gaps");

const speedSnapshot: TimelineSnapshot = {
  duration_s: 15,
  tracks: [
    {
      kind: "video",
      role: "primary",
      items: [
        {
          kind: "clip",
          index: 0,
          name: "fast",
          clip_uuid: "u-fast",
          track_start_s: 0,
          duration_s: 5,
          asset_id: "raw/fast.mov",
          source_start_s: 40,
          proxy_path: null,
          playable_path: "/project/raw/fast.mov",
          playable_kind: "source",
          thumbnail_dir: null,
          waveform_path: null,
          volume: 1,
          speed: 2,
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
          lut_strength: null,
          title: null,
          video_overlay: null,
          animations: [],
          transition_id: null,
          effect_name: null,
          in_offset_s: null,
          out_offset_s: null,
        },
        {
          kind: "clip",
          index: 1,
          name: "slow",
          clip_uuid: "u-slow",
          track_start_s: 5,
          duration_s: 10,
          asset_id: "raw/slow.mov",
          source_start_s: 100,
          proxy_path: null,
          playable_path: "/project/raw/slow.mov",
          playable_kind: "source",
          thumbnail_dir: null,
          waveform_path: null,
          volume: 1,
          speed: 0.5,
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
          lut_strength: null,
          title: null,
          video_overlay: null,
          animations: [],
          transition_id: null,
          effect_name: null,
          in_offset_s: null,
          out_offset_s: null,
        },
      ],
    },
  ],
} as unknown as TimelineSnapshot;

const speedPlan = derivePreviewPlan(speedSnapshot);
const fast = speedPlan.segments[0];
const slow = speedPlan.segments[1];
assert(fast.sourceEnd === 50, "2x clip should consume 10s of source in 5s");
assert(slow.sourceEnd === 105, "0.5x clip should consume 5s of source in 10s");
assert(
  sourceTimeForTimelineTime(fast, 3) === 46,
  "2x clip should map timeline delta 3s to source delta 6s",
);
assert(
  sourceTimeForTimelineTime(slow, 15) === 105,
  "0.5x clip should map timeline delta 10s to source delta 5s",
);
assert(
  timelineTimeForSource(speedPlan.segments, "fast", 46) === 3,
  "source-to-timeline mapping should divide by speed",
);
assert(
  nearestTimelineTimeForSource(speedPlan.segments, "slow", 110) === 15,
  "nearest source mapping should clamp then divide by speed",
);
console.log("ok  preview segment time mapping respects clip speed");
