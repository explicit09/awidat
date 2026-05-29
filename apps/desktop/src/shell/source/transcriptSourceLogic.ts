/**
 * Pure functions consumed by `<TranscriptSource>`. Lifted out of the
 * component so they can be unit-tested under plain node, and so the
 * component stays focused on layout + side effects.
 *
 *   - `buildDeleteRangeOpsForStem` mirrors the existing helper in
 *     `TranscriptView.tsx` (which is module-private there). We
 *     duplicate the contract here rather than refactor the load-
 *     bearing transcript surface; that keeps Wave 4 W4.4 strictly
 *     additive.
 *
 *   - `isRangeCutFromTimeline` answers "is this source-time range gone
 *     from the cut?" — used by the AppliedCutsOverlay to dim the
 *     transcript spans that the agent's accepted cuts have removed.
 */

import type { ProjectType, TimelineSnapshot } from "../../protocol";
import type { EdlOp } from "../../timeline/edlBuilder";

/* -------------------------- routing decision ------------------------ */

/**
 * Project types that should land on the transcript-first Source view.
 * Other types (shorts / other) keep the legacy video preview.
 *
 * `interview` isn't an explicit `ProjectType.kind` today — it shows
 * up as an `"other"` description in v1. We bias toward routing
 * `"other"` projects whose description hints at interview/podcast
 * content, but the safe default is to keep `"other"` on the legacy
 * surface. Wave 5 may add `interview` as a first-class kind.
 */
export function isTranscriptFirstProjectType(pt: ProjectType | null): boolean {
  if (!pt) return false;
  return pt.kind === "podcast" || pt.kind === "tutorial";
}

/* ---------------------------- propose delete ------------------------ */

interface BuildDeleteArgs {
  snapshot: TimelineSnapshot;
  stem: string;
  sourceStart: number;
  sourceEnd: number;
}

/**
 * Build an EDL envelope to cut `[sourceStart, sourceEnd]` of asset
 * `stem` out of the timeline. Mirrors the v1 contract from
 * `TranscriptView.tsx`'s private helper:
 *
 *   - Range must fully sit inside a single video-track clip. Multi-
 *     clip spans return `[]` (caller surfaces a warning).
 *   - Empty / inverted ranges return `[]`.
 *   - Returns: `Split @ start; Split @ end; Delete (middle piece)`.
 */
export function buildDeleteRangeOpsForStem(args: BuildDeleteArgs): EdlOp[] {
  const { snapshot, stem, sourceStart, sourceEnd } = args;
  if (!(sourceEnd > sourceStart + 0.05)) return [];

  const candidates: {
    clipUuid: string;
    clipSourceStart: number;
    clipSourceEnd: number;
  }[] = [];
  for (const track of snapshot.tracks) {
    if (track.kind !== "video") continue;
    for (const item of track.items) {
      if (item.kind !== "clip") continue;
      if (item.proxy_path === null) continue;
      if (stemOf(item.proxy_path) !== stem) continue;
      const clipStart = item.source_start_s ?? 0;
      const clipEnd = clipStart + item.duration_s;
      candidates.push({
        clipUuid: item.clip_uuid,
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

  return [
    {
      kind: "split_clip",
      anchor: { kind: "clip_uuid", uuid: fullyInside.clipUuid },
      atS: sourceStart,
    },
    {
      kind: "split_clip",
      anchor: { kind: "clip_uuid", uuid: nameAfterSplit(fullyInside.clipUuid) },
      atS: sourceEnd,
    },
    {
      kind: "delete_clip",
      anchor: { kind: "clip_uuid", uuid: nameAfterSplit(fullyInside.clipUuid) },
    },
  ];
}

/* ------------------------ accepted-cut detection -------------------- */

/**
 * Return true when `[sourceStart, sourceEnd]` for `stem` has NO
 * overlap with any clip on the timeline. Used by AppliedCutsOverlay
 * to dim transcript spans the agent's accepted cuts have removed.
 *
 * Semantics:
 *   - Empty timeline (no clips for stem) → false. This is the pre-
 *     timeline source-preview case; we don't want to grey out the
 *     entire transcript just because the asset hasn't been placed yet.
 *   - At least one clip exists for stem, but this range lands in a
 *     gap between clips → true (the cut was applied).
 *   - Range partially overlaps a clip → false (still visible in the
 *     cut, even if trimmed).
 */
export function isRangeCutFromTimeline(
  snapshot: TimelineSnapshot,
  stem: string,
  sourceStart: number,
  sourceEnd: number,
): boolean {
  const stemClips: { start: number; end: number }[] = [];
  for (const track of snapshot.tracks) {
    if (track.kind !== "video") continue;
    for (const item of track.items) {
      if (item.kind !== "clip") continue;
      if (item.proxy_path === null) continue;
      if (stemOf(item.proxy_path) !== stem) continue;
      const start = item.source_start_s ?? 0;
      stemClips.push({ start, end: start + item.duration_s });
    }
  }
  if (stemClips.length === 0) return false;
  const mid = (sourceStart + sourceEnd) / 2;
  // Cheap point-in-clip test: the word's midpoint sits inside at
  // least one of the stem's clip windows. Half-second tolerance on
  // both ends so a word that exactly aligns with a clip boundary
  // doesn't flicker between "in" and "out".
  for (const c of stemClips) {
    if (mid >= c.start - 0.05 && mid <= c.end + 0.05) return false;
  }
  return true;
}

/* -------------------------------- helpers --------------------------- */

function nameAfterSplit(parent: string): string {
  return `${parent}-b`;
}

function stemOf(path: string): string {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const file = slash >= 0 ? path.slice(slash + 1) : path;
  const dot = file.lastIndexOf(".");
  return dot > 0 ? file.slice(0, dot) : file;
}
