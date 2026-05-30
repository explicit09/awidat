import { strict as assert } from "node:assert";
import { getStarterPrompts } from "../src/agent/starterPrompts.ts";

const podcast = getStarterPrompts("podcast");
assert.equal(podcast.length, 4);
assert.ok(podcast.every((p) => typeof p === "string" && p.length > 0));

const unknown = getStarterPrompts("totally_unknown_type_42");
assert.equal(unknown.length, 4);
assert.ok(unknown.includes("Suggest a starting edit"));

console.log("starter-prompts: OK");
