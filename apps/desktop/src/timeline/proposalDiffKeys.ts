import type { AppliedDiff } from "../protocol";

/** Build keys for original-snapshot items being deleted by a proposal. */
export function collectDeletedKeys(diffs: AppliedDiff[]): Set<string> {
  const out = new Set<string>();
  for (const diff of diffs) {
    if (diff.kind === "delete") {
      out.add(`${diff.track_index}:${diff.item_index}`);
    }
  }
  return out;
}

/** Build keys for proposed-snapshot items changed by a proposal. */
export function collectHighlightKeys(diffs: AppliedDiff[]): Set<string> {
  const out = new Set<string>();
  for (const diff of diffs) {
    if (diff.kind === "trim_edge") {
      out.add(`${diff.track_index}:${diff.item_index}`);
    } else if (diff.kind === "split") {
      out.add(`${diff.track_index}:${diff.item_index}`);
      out.add(`${diff.track_index}:${diff.item_index + 1}`);
    } else if (
      diff.kind === "insert" ||
      diff.kind === "insert_b_roll" ||
      diff.kind === "insert_pi_p"
    ) {
      out.add(`${diff.track_index}:${diff.item_index}`);
    } else if (diff.kind === "move") {
      out.add(`${diff.to_track_index}:${diff.to_item_index}`);
    }
  }
  return out;
}
