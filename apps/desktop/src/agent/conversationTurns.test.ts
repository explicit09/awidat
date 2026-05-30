import assert from "node:assert/strict";
import { itemsToConversationTurns } from "./conversationTurns.ts";
import type { Item } from "../protocol";

const items: Item[] = [
  { kind: "user_input", id: "user-1", text: "Can you inspect this?" },
  {
    kind: "approval_request",
    id: "approval-1",
    phase: "started",
    tool_name: "bash",
    args_summary: "cargo test -p awidat-desktop",
    capability_metadata: { side_effects: ["runs command"] },
  },
];

const turns = itemsToConversationTurns(items, () => "tool summary");

assert.equal(turns.length, 1);
assert.equal(turns[0].parts.length, 1);
assert.deepEqual(turns[0].parts[0], {
  kind: "approval_request",
  id: "approval-1",
  toolName: "bash",
  argsSummary: "cargo test -p awidat-desktop",
  capabilityMetadata: { side_effects: ["runs command"] },
});
