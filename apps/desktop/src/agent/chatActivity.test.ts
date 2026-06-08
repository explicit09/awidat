import assert from "node:assert/strict";
import type { Item } from "../protocol";
import {
  activityGroupLabel,
  activityStatus,
  groupActivityItems,
  jobLabel,
  jobResultText,
} from "./chatActivity.ts";

const items: Item[] = [
  { kind: "user_input", id: "user-1", text: "hello" },
  {
    kind: "job",
    id: "job-1",
    phase: "completed",
    job_kind: "transcode",
    percent: 100,
    status: "proxy ready",
    result: { ok: { summary: "proxy ready" } },
    output_path: "/tmp/proxy.mp4",
  },
  {
    kind: "tool_call",
    id: "tool-1",
    phase: "completed",
    name: "view_timeline",
    args: {},
    result: { Ok: "timeline read" },
  },
  {
    kind: "text",
    id: "answer-1",
    phase: "completed",
    text: "Hello. What would you like to edit?",
  },
];

const grouped = groupActivityItems(items);

assert.deepEqual(
  grouped.map((entry) => entry.kind),
  ["item", "activity_group", "item"],
  "adjacent jobs and tools should become one compact activity group",
);

const activity = grouped[1];
assert.equal(activity.kind, "activity_group");
assert.equal(activityGroupLabel(activity.items), "1 job · 1 tool call");
assert.equal(activityStatus(activity.items[0]), "done");
assert.equal(jobLabel(activity.items[0].kind === "job" ? activity.items[0].job_kind : "transcode"), "transcode");
assert.equal(
  activity.items[0].kind === "job" ? jobResultText(activity.items[0]) : "",
  "proxy ready",
);

console.log("chat-activity: OK");
