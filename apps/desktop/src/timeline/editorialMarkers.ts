import type { TimelineItem, TimelineSnapshot } from "./store.ts";
import { LANE_HEIGHT, RULER_HEIGHT } from "./layout.ts";

export type EditorialMarker = {
  key: string;
  x: number;
  y: number;
  label: string;
  title: string;
};

export function buildCutBadges(
  snapshot: TimelineSnapshot,
  pps: number,
): EditorialMarker[] {
  const out: EditorialMarker[] = [];
  for (const boundary of snapshot.cut_boundaries) {
    const located = locateClipByUuid(snapshot, boundary.to_clip_id);
    if (!located) continue;
    out.push({
      key: `cut-${boundary.key}`,
      x: Math.max(2, located.item.track_start_s * pps - 10),
      y: RULER_HEIGHT + located.trackIndex * LANE_HEIGHT + 2,
      label: shortCutLabel(boundary.cut_type),
      title: [
        formatEditorialLabel(boundary.cut_type),
        boundary.intent ? `intent: ${boundary.intent}` : null,
        boundary.audio_relation ? `audio: ${boundary.audio_relation}` : null,
        boundary.reason,
      ]
        .filter(Boolean)
        .join(" - "),
    });
  }
  return out;
}

export function buildSplitOffsets(
  snapshot: TimelineSnapshot,
  pps: number,
): EditorialMarker[] {
  const out: EditorialMarker[] = [];
  for (let trackIndex = 0; trackIndex < snapshot.tracks.length; trackIndex += 1) {
    const track = snapshot.tracks[trackIndex];
    for (const item of track.items) {
      if (item.kind !== "clip") continue;
      const y = RULER_HEIGHT + trackIndex * LANE_HEIGHT + LANE_HEIGHT - 18;
      if (item.audio_lead_s !== null && item.audio_lead_s > 0) {
        out.push({
          key: `lead-${item.clip_uuid}`,
          x: Math.max(2, item.track_start_s * pps + 4),
          y,
          label: `J +${formatMarkerSeconds(item.audio_lead_s)}`,
          title: splitOffsetTitle("Audio lead", item.audio_lead_s, item),
        });
      }
      if (item.audio_trail_s !== null && item.audio_trail_s > 0) {
        out.push({
          key: `trail-${item.clip_uuid}`,
          x: Math.max(2, (item.track_start_s + item.duration_s) * pps - 50),
          y,
          label: `L +${formatMarkerSeconds(item.audio_trail_s)}`,
          title: splitOffsetTitle("Audio trail", item.audio_trail_s, item),
        });
      }
    }
  }
  return out;
}

function locateClipByUuid(snapshot: TimelineSnapshot, clipUuid: string) {
  for (let trackIndex = 0; trackIndex < snapshot.tracks.length; trackIndex += 1) {
    const item = snapshot.tracks[trackIndex].items.find(
      (candidate) => candidate.kind === "clip" && candidate.clip_uuid === clipUuid,
    );
    if (item?.kind === "clip") return { trackIndex, item };
  }
  return null;
}

function shortCutLabel(cutType: string): string {
  switch (cutType) {
    case "cut_on_action":
      return "Action";
    case "shot_reverse_shot":
      return "S/RS";
    case "eyeline_match_cut":
      return "Eye";
    case "match_cut":
      return "Match";
    case "smash_cut":
      return "Smash";
    case "cross_cut":
      return "Cross";
    default:
      return formatEditorialLabel(cutType);
  }
}

export function formatEditorialLabel(value: string): string {
  return value
    .split("_")
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

export function formatMarkerSeconds(value: number): string {
  return `${value.toFixed(2).replace(/\.?0+$/, "")}s`;
}

function splitOffsetTitle(
  label: string,
  seconds: number,
  item: Extract<TimelineItem, { kind: "clip" }>,
): string {
  return [
    `${label}: ${formatMarkerSeconds(seconds)}`,
    item.split_edit_reason,
    item.split_edit_confidence !== null
      ? `confidence ${Math.round(item.split_edit_confidence * 100)}%`
      : null,
  ]
    .filter(Boolean)
    .join(" - ");
}
