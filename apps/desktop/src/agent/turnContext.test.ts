import assert from "node:assert/strict";
import {
  buildTurnContext,
  chatHistoryLoader,
  type ContextChip,
  type ChatSessionSummary,
} from "./turnContext.ts";

const chips: ContextChip[] = [
  { label: "Project: Episode 12", kind: "project" },
  { label: "Clip: intro-cam-a", kind: "media" },
  { label: "Timeline: 8:03", kind: "selection" },
  { label: "Proposal: remove filler words", kind: "lens" },
];

assert.deepEqual(buildTurnContext(chips), [
  "project: Episode 12",
  "media: Clip: intro-cam-a",
  "selection: Timeline: 8:03",
  "lens: Proposal: remove filler words",
]);

assert.deepEqual(
  buildTurnContext([
    { label: "Project: Episode 12", kind: "project" },
    { label: "  ", kind: "media" },
    { label: "Clip: intro-cam-a", kind: "media" },
  ]),
  ["project: Episode 12", "media: Clip: intro-cam-a"],
);

const calls: string[] = [];
const session: ChatSessionSummary = {
  id: "thread-a",
  title: "Chat A",
  projectRoot: "/tmp/project",
  logPath: "/tmp/rollout-a.jsonl",
  startedAt: "2026-05-28T00:00:00Z",
  messageCount: 2,
};

await chatHistoryLoader(
  async (command: string, args?: Record<string, unknown>) => {
    calls.push(`${command}:${String(args?.logPath ?? "")}`);
    return { session, items: [] };
  },
  session,
);

assert.deepEqual(calls, ["resume_chat_session:/tmp/rollout-a.jsonl"]);
