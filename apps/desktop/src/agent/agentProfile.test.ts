import assert from "node:assert/strict";

import { AGENT_PROFILE_LABELS, AGENT_PROFILE_OPTIONS } from "./agentProfile.ts";

assert.deepEqual(AGENT_PROFILE_OPTIONS.map((option) => option.value), [
  "balanced",
  "deep_edit",
]);
assert.equal(AGENT_PROFILE_LABELS.balanced, "Balanced");
assert.equal(AGENT_PROFILE_LABELS.deep_edit, "Deep Edit");
assert.match(AGENT_PROFILE_OPTIONS[0].description, /routine/i);
assert.match(AGENT_PROFILE_OPTIONS[1].description, /story|visual/i);

console.log("agent profile contract passed");
