import type { TimelineSnapshot } from "../../protocol";
import type { EdlOp } from "../../timeline/edlBuilder";

interface BuildDeleteArgs {
  snapshot: TimelineSnapshot;
  stem: string;
  sourceStart: number;
  sourceEnd: number;
  clipUuid?: string;
}

/**
 * Build an EDL envelope to cut `[sourceStart, sourceEnd]` of asset
 * `stem` out of the timeline for TranscriptView:
 *
 *   - Range must fully sit inside a single video-track clip. Multi-
 *     clip spans return `[]` (caller surfaces a warning).
 *   - Empty / inverted ranges return `[]`.
 *   - `clipUuid`, when provided, constrains the cut to the selected
 *     timeline occurrence rather than the first matching source range.
 *   - Returns: `Split @ start; Split @ end; Ripple Delete (middle piece)`.
 */
export function buildDeleteRangeOpsForStem(args: BuildDeleteArgs): EdlOp[] {
  const { snapshot, stem, sourceStart, sourceEnd, clipUuid } = args;
  if (!(sourceEnd > sourceStart + 0.05)) return [];

  const candidates: {
    clipUuid: string;
    clipName: string;
    clipSourceStart: number;
    clipSourceEnd: number;
  }[] = [];
  for (const track of snapshot.tracks) {
    if (track.kind !== "video") continue;
    for (const item of track.items) {
      if (item.kind !== "clip") continue;
      // When the caller knows the selected timeline clip, match on its
      // stable id alone — the clip may be source-playable while its
      // proxy is still generating (`proxy_path === null`), in which
      // case the stem check below would wrongly skip it.
      if (clipUuid !== undefined) {
        if (item.clip_uuid !== clipUuid) continue;
      } else if (clipPlayableStem(item) !== stem) {
        continue;
      }
      const clipStart = item.source_start_s ?? 0;
      const clipEnd = clipStart + item.duration_s;
      candidates.push({
        clipUuid: item.clip_uuid,
        clipName: item.name,
        clipSourceStart: clipStart,
        clipSourceEnd: clipEnd,
      });
    }
  }
  if (candidates.length === 0) return [];

  const fullyInside = candidates.find(
    (c) =>
      c.clipSourceStart <= sourceStart + 0.01 &&
      c.clipSourceEnd >= sourceEnd - 0.01,
  );
  if (!fullyInside) return [];

  // `apply_split` names the right piece `${name}-b` and stamps it with
  // a *fresh* clip_uuid. So the follow-up ops must anchor on the
  // name-derived value, not `${clip_uuid}-b` — the resolver matches a
  // ClipUuid anchor against `name` as well as `clip_uuid`, so this is
  // correct for both stamped (clip_uuid != name) and un-stamped clips.
  const rightPiece = nameAfterSplit(fullyInside.clipName);

  return [
    {
      kind: "split_clip",
      anchor: { kind: "clip_uuid", uuid: fullyInside.clipUuid },
      atS: sourceStart,
    },
    {
      kind: "split_clip",
      anchor: { kind: "clip_uuid", uuid: rightPiece },
      atS: sourceEnd,
    },
    {
      kind: "ripple_delete",
      anchor: { kind: "clip_uuid", uuid: rightPiece },
    },
  ];
}

/* -------------------------------- helpers --------------------------- */

function nameAfterSplit(parent: string): string {
  return `${parent}-b`;
}

/**
 * The proxy/source stem backing a clip, matching the `proxyStem` that
 * `usePlaySegments` derives. Prefers `proxy_path`, but falls back to
 * `playable_path` so a clip playing from its original source (proxy
 * still generating, `proxy_path === null`) still resolves to the same
 * stem the composed transcript selected it by. Returns null when the
 * clip has no playable media at all.
 */
function clipPlayableStem(item: {
  proxy_path: string | null;
  playable_path?: string;
}): string | null {
  const path = item.proxy_path ?? item.playable_path ?? null;
  return path === null ? null : stemOf(path);
}

function stemOf(path: string): string {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const file = slash >= 0 ? path.slice(slash + 1) : path;
  const dot = file.lastIndexOf(".");
  return dot > 0 ? file.slice(0, dot) : file;
}
