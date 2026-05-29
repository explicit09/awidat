/**
 * Pure-logic tests for the per-project skills disable store.
 * Exercises the helpers without spinning up a Zustand store or DOM.
 */
import { strict as assert } from "node:assert";
import {
  applySetDisabled,
  applyToggle,
  computeIsDisabled,
  deserialize,
  serialize,
} from "../src/state/skills.ts";

// Defaults: nothing disabled.
{
  const empty = new Map<string, Set<string>>();
  assert.equal(computeIsDisabled(empty, "/p", "auto-cutter"), false);
  assert.equal(computeIsDisabled(empty, null, "auto-cutter"), false);
}

// Toggle on then off — symmetric.
{
  let state = new Map<string, Set<string>>();
  state = applyToggle(state, "/p", "auto-cutter");
  assert.equal(computeIsDisabled(state, "/p", "auto-cutter"), true);
  state = applyToggle(state, "/p", "auto-cutter");
  assert.equal(computeIsDisabled(state, "/p", "auto-cutter"), false);
  // After both toggles, the project key should be gone (no leftover empty set).
  assert.equal(state.has("/p"), false);
}

// Per-project isolation — disabling in one project leaves the other alone.
{
  let state = new Map<string, Set<string>>();
  state = applyToggle(state, "/proj-a", "b-roll-suggester");
  assert.equal(computeIsDisabled(state, "/proj-a", "b-roll-suggester"), true);
  assert.equal(computeIsDisabled(state, "/proj-b", "b-roll-suggester"), false);
}

// setDisabled is idempotent and explicit.
{
  let state = new Map<string, Set<string>>();
  state = applySetDisabled(state, "/p", "auto-cutter", true);
  state = applySetDisabled(state, "/p", "auto-cutter", true);
  assert.equal(state.get("/p")?.size, 1);
  state = applySetDisabled(state, "/p", "auto-cutter", false);
  assert.equal(state.has("/p"), false);
}

// serialize / deserialize is a lossless round-trip.
{
  let state = new Map<string, Set<string>>();
  state = applyToggle(state, "/p1", "skill-a");
  state = applyToggle(state, "/p1", "skill-b");
  state = applyToggle(state, "/p2", "skill-a");

  const shape = serialize(state);
  // Serialized arrays should be sorted for stable persistence.
  assert.deepEqual(shape.disabled["/p1"], ["skill-a", "skill-b"]);
  assert.deepEqual(shape.disabled["/p2"], ["skill-a"]);

  const restored = deserialize(shape);
  assert.equal(restored.get("/p1")?.has("skill-a"), true);
  assert.equal(restored.get("/p1")?.has("skill-b"), true);
  assert.equal(restored.get("/p2")?.has("skill-a"), true);
  assert.equal(restored.get("/p2")?.has("skill-b"), false);
}

// Deserialize is defensive about missing / malformed input.
{
  assert.equal(deserialize(undefined).size, 0);
  // Empty arrays don't materialize as empty sets.
  assert.equal(deserialize({ disabled: { "/p": [] } }).size, 0);
}

// `null` project root collapses to a shared "__global__" bucket — used
// when no project is loaded but the user still wants their toggle to
// stick. Confirm the two read paths agree.
{
  let state = new Map<string, Set<string>>();
  state = applyToggle(state, null, "auto-cutter");
  assert.equal(computeIsDisabled(state, null, "auto-cutter"), true);
  // The actual key is opaque to callers — just verify it exists.
  assert.equal(state.size, 1);
}

console.log("skills-store: OK");
