import { derivePreviewPlan } from "../src/timeline/usePlaySegments";
import type { TimelineSnapshot } from "../src/timeline/store";

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const snapshot = {
  duration_s: 10,
  broadcast_overlay: null,
  cut_boundaries: [],
  preview_limitations: [],
  tracks: [
    {
      name: "V1",
      kind: "video",
      role: null,
      audio: null,
      items: [
        {
          kind: "clip",
          index: 0,
          clip_uuid: "video-clip",
          name: "video",
          asset_id: "asset-1",
          proxy_path: "/tmp/source-proxy.mp4",
          playable_path: "/tmp/source-proxy.mp4",
          playable_kind: "proxy",
          thumbnail_dir: null,
          waveform_path: null,
          track_start_s: 0,
          duration_s: 10,
          source_start_s: 0,
          speed: null,
          volume: null,
          fade_in_s: null,
          fade_out_s: null,
          audio_lead_s: null,
          audio_trail_s: null,
          split_edit_reason: null,
          split_edit_confidence: null,
          link_group_id: "linked-1",
          has_video: true,
          has_audio: true,
          color_correction: null,
          lut_path: null,
          title: null,
          video_overlay: null,
          animations: [],
        },
      ],
    },
    {
      name: "A1",
      kind: "audio",
      role: "dialogue",
      audio: {
        volume: 1,
        muted: false,
        solo: false,
        role: "dialogue",
        ducking: null,
      },
      items: [],
    },
  ],
} satisfies TimelineSnapshot;

const plan = derivePreviewPlan(snapshot);

assert(plan.segments.length === 1, "expected one playable segment");
assert(plan.segments[0]?.volume === 1, "linked audio track should not mute preview playback");
