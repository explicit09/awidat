import type { PlayableKind } from "../protocol";

/**
 * One contiguous range on the timeline along with its cache state.
 * The ruler renders each range as a colored slice — proxy green,
 * source amber, missing red.
 */
export type CacheStripInput = {
  startS: number;
  endS: number;
  kind: PlayableKind;
};

/**
 * Merge consecutive same-kind ranges into a single strip segment so
 * the ruler doesn't render dozens of tiny rectangles for adjacent
 * proxy-backed clips. A gap (non-abutting ranges) or a kind change
 * starts a new strip segment.
 *
 * Treats start/end alignment as exact within ~1ms — floating-point
 * accumulated drift shouldn't split a run when the user's intent is
 * obviously "these clips touch."
 */
export function cacheStripSegments(clips: CacheStripInput[]): CacheStripInput[] {
  const out: CacheStripInput[] = [];
  for (const clip of clips) {
    const prev = out[out.length - 1];
    if (prev && prev.kind === clip.kind && Math.abs(prev.endS - clip.startS) < 0.001) {
      prev.endS = clip.endS;
    } else {
      out.push({ ...clip });
    }
  }
  return out;
}
