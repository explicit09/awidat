import assert from "node:assert/strict";
import type { Item } from "../src/protocol/index.ts";
import { useAgentStore } from "../src/agent/store.ts";

function resetStore() {
  useAgentStore.getState().replace([]);
}

resetStore();

const optimistic: Item = {
  kind: "user_input",
  id: "optimistic-user-1",
  text: "Trim the first half.",
};
const backend: Item = {
  kind: "user_input",
  id: "backend-user-1",
  text: "Trim the first half.",
};

useAgentStore.getState().upsert(optimistic);
useAgentStore.getState().upsert(backend);

assert.deepEqual(useAgentStore.getState().items, [backend]);

useAgentStore.getState().upsert({
  kind: "user_input",
  id: "backend-user-2",
  text: "Add captions.",
});

assert.equal(useAgentStore.getState().items.length, 2);

console.log("agent-store-optimistic: OK");
