import assert from "node:assert/strict";
import { collectDeletedKeys, collectHighlightKeys } from "./proposalDiffKeys.ts";
import type { AppliedDiff } from "../protocol";

const diffs: AppliedDiff[] = [
  { kind: "delete", op_index: 0, track_index: 1, item_index: 2 },
  {
    kind: "trim_edge",
    op_index: 1,
    track_index: 0,
    item_index: 3,
    side: "start",
    delta_s: 0.5,
  },
  { kind: "split", op_index: 2, track_index: 0, item_index: 4, at_s: 7 },
  { kind: "insert", op_index: 3, track_index: 2, item_index: 0 },
  { kind: "insert_b_roll", op_index: 4, track_index: 2, item_index: 1 },
  { kind: "insert_pi_p", op_index: 5, track_index: 3, item_index: 0 },
  {
    kind: "move",
    op_index: 6,
    from_track_index: 0,
    from_item_index: 1,
    to_track_index: 4,
    to_item_index: 2,
  },
];

console.log("# collectDeletedKeys");
assert.deepEqual([...collectDeletedKeys(diffs)].sort(), ["1:2"]);
console.log("  ok  marks only deleted original items");

console.log("# collectHighlightKeys");
assert.deepEqual([...collectHighlightKeys(diffs)].sort(), [
  "0:3",
  "0:4",
  "0:5",
  "2:0",
  "2:1",
  "3:0",
  "4:2",
]);
console.log("  ok  marks proposed-state edits including both split halves and move target");
