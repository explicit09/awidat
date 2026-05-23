import type { Item } from "../protocol";

export function countCompletedTimelineEdits(items: Item[]): number {
  return (
    items.filter(
      (item) =>
        item.kind === "tool_call" &&
        item.name === "apply_edl" &&
        item.phase === "completed",
    ).length +
    items.filter(
      (item) =>
        item.kind === "proposed_edit" &&
        item.phase === "completed" &&
        item.source.source === "user" &&
        item.snapshot.tracks.length > 0,
    ).length
  );
}
