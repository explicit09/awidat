import assert from "node:assert/strict";
import { countCompletedTimelineEdits } from "./refreshActivity.ts";
import type { Item, TimelineSnapshot } from "../protocol";

const emptySnapshot: TimelineSnapshot = {
  duration_s: 0,
  broadcast_overlay: null,
  cut_boundaries: [],
  preview_limitations: [],
  tracks: [],
};

const populatedSnapshot: TimelineSnapshot = {
  ...emptySnapshot,
  duration_s: 4,
  tracks: [{ name: "V1", kind: "video", role: null, audio: null, items: [] }],
};

const items: Item[] = [
  {
    kind: "tool_call",
    id: "tool-1",
    phase: "completed",
    name: "apply_edl",
    args: {},
    result: { Ok: "ok" },
  },
  {
    kind: "tool_call",
    id: "tool-2",
    phase: "completed",
    name: "find_moment",
    args: {},
    result: { Ok: "ok" },
  },
  {
    kind: "tool_call",
    id: "tool-3",
    phase: "started",
    name: "apply_edl",
    args: {},
    result: null,
  },
  {
    kind: "proposed_edit",
    id: "proposal-1",
    phase: "completed",
    source: { source: "user" },
    edl_text: "",
    snapshot: populatedSnapshot,
    diff_hints: [],
    summary: "user trim",
    revision: 1,
  },
  {
    kind: "proposed_edit",
    id: "proposal-2",
    phase: "completed",
    source: { source: "agent", call_id: "call-1", tool_name: "apply_edl" },
    edl_text: "",
    snapshot: populatedSnapshot,
    diff_hints: [],
    summary: "agent trim",
    revision: 1,
  },
  {
    kind: "proposed_edit",
    id: "proposal-3",
    phase: "completed",
    source: { source: "user" },
    edl_text: "",
    snapshot: emptySnapshot,
    diff_hints: [],
    summary: "empty user proposal",
    revision: 1,
  },
];

console.log("# countCompletedTimelineEdits");
assert.equal(countCompletedTimelineEdits(items), 2);
console.log("  ok  counts completed apply_edl calls and populated user proposals only");
