import { derivePreviewPlan } from "../src/timeline/usePlaySegments";
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
assert(plan.segments.length === 1, "clip with playable source must be in segments");
assert(
  plan.segments[0].proxyPath === "/project/raw/clip.mov",
  "segment must carry the source-fallback path",
);
console.log("ok  play-segments include source-fallback clips");
