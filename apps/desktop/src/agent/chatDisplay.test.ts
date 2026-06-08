import assert from "node:assert/strict";
import type { Item } from "../protocol";
import { INTRO_PROMPT, PREPARE_PROMPT, LEGACY_AWIDAT_INTRO_PROMPT_PREFIX } from "../state/introState.ts";
import { visibleChatItems } from "./chatDisplay.ts";

const introItems: Item[] = [
  { kind: "user_input", id: "intro-user", text: INTRO_PROMPT },
  {
    kind: "text",
    id: "intro-answer",
    phase: "completed",
    text: "I've read AGENTS.md and the available skills.",
  },
  { kind: "user_input", id: "real-user", text: "hello" },
  {
    kind: "text",
    id: "real-answer",
    phase: "completed",
    text: "Tell me what edit pass you want.",
  },
];

assert.deepEqual(
  visibleChatItems(introItems).map((item) => item.id),
  ["real-user", "real-answer"],
  "synthetic intro prompt and intro answer should be hidden",
);

const legacyIntroItems: Item[] = [
  {
    kind: "user_input",
    id: "legacy-intro-user",
    text: `${LEGACY_AWIDAT_INTRO_PROMPT_PREFIX}\nYou've just been opened on a project.`,
  },
  {
    kind: "text",
    id: "legacy-intro-answer",
    phase: "completed",
    text: "I've read AGENTS.md.",
  },
  { kind: "user_input", id: "real-after-legacy", text: "hello" },
];

assert.deepEqual(
  visibleChatItems(legacyIntroItems).map((item) => item.id),
  ["real-after-legacy"],
  "legacy awidat intro prompt and intro answer should be hidden",
);

const prepareItems: Item[] = [
  { kind: "user_input", id: "prepare-user", text: PREPARE_PROMPT },
  {
    kind: "text",
    id: "prepare-answer",
    phase: "completed",
    text: "I found three proposed edits.",
  },
];

assert.deepEqual(
  visibleChatItems(prepareItems).map((item) => item.id),
  ["prepare-answer"],
  "prepare output should remain visible while its synthetic prompt is hidden",
);

console.log("chat-display: OK");
