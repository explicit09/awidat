/**
 * Pure-logic tests for the per-project skills disable store.
 * Exercises the helpers without spinning up a Zustand store or DOM.
 *
 * Wave 4 T1 added the on-disk persistence — write-on-toggle flushes
 * the project's disabled set to `<project>/.awidat/skills.json` via a
 * Tauri command. The store exposes a `__setPersistDisabledForTests`
 * seam so we can verify the invocation without mocking the IPC bridge.
 */
import { strict as assert } from "node:assert";
import {
  __setPersistDisabledForTests,
  applyHydrate,
  applySetDisabled,
  applyToggle,
  computeIsDisabled,
  deserialize,
  serialize,
  useSkillsStore,
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

// applyHydrate replaces the project's set wholesale and clears the
// project key when the disk says "nothing disabled".
{
  let state = new Map<string, Set<string>>();
  state = applyHydrate(state, "/p", ["a", "b"]);
  assert.equal(state.get("/p")?.has("a"), true);
  assert.equal(state.get("/p")?.has("b"), true);
  // Re-hydrating with an empty list drops the project key.
  state = applyHydrate(state, "/p", []);
  assert.equal(state.has("/p"), false);
}

// applyHydrate replaces, does not merge. A skill that was disabled in
// the in-memory cache but absent from disk should end up enabled
// after hydrate.
{
  let state = new Map<string, Set<string>>();
  state = applySetDisabled(state, "/p", "stale-skill", true);
  state = applyHydrate(state, "/p", ["fresh-skill"]);
  assert.equal(state.get("/p")?.has("stale-skill"), false);
  assert.equal(state.get("/p")?.has("fresh-skill"), true);
}

// Store-level: toggle on a real project root invokes the persistence
// sink with the deduped + sorted disabled list. Toggle off then writes
// an empty list.
{
  type Call = { projectRoot: string; disabled: string[] };
  const calls: Call[] = [];
  const restore = __setPersistDisabledForTests(async (projectRoot, disabled) => {
    calls.push({ projectRoot, disabled: [...disabled] });
  });
  try {
    // Reset the store so prior persisted localStorage state doesn't
    // bleed into our assertions.
    useSkillsStore.setState({ disabledByProject: new Map() });
    useSkillsStore.getState().toggle("/proj-a", "auto-cutter");
    assert.equal(calls.length, 1, "first toggle should persist");
    assert.equal(calls[0]?.projectRoot, "/proj-a");
    assert.deepEqual(calls[0]?.disabled, ["auto-cutter"]);
    useSkillsStore.getState().toggle("/proj-a", "b-roll-suggester");
    assert.equal(calls.length, 2);
    assert.deepEqual(calls[1]?.disabled, ["auto-cutter", "b-roll-suggester"]);
    // Toggling off auto-cutter persists the remaining single name.
    useSkillsStore.getState().toggle("/proj-a", "auto-cutter");
    assert.equal(calls.length, 3);
    assert.deepEqual(calls[2]?.disabled, ["b-roll-suggester"]);
  } finally {
    __setPersistDisabledForTests(restore);
  }
}

// setDisabled flows through the same sink — verifies both the toggle
// and explicit-set paths invoke persistence consistently.
{
  type Call = { projectRoot: string; disabled: string[] };
  const calls: Call[] = [];
  const restore = __setPersistDisabledForTests(async (projectRoot, disabled) => {
    calls.push({ projectRoot, disabled: [...disabled] });
  });
  try {
    useSkillsStore.setState({ disabledByProject: new Map() });
    useSkillsStore.getState().setDisabled("/proj-x", "skill-one", true);
    assert.equal(calls.length, 1);
    assert.deepEqual(calls[0]?.disabled, ["skill-one"]);
    useSkillsStore.getState().setDisabled("/proj-x", "skill-one", false);
    assert.equal(calls.length, 2);
    // Empty list = nothing disabled after the off toggle.
    assert.deepEqual(calls[1]?.disabled, []);
  } finally {
    __setPersistDisabledForTests(restore);
  }
}

// Null project root must NOT trigger persistence — the global bucket
// has no on-disk counterpart and writing through `null` would crash
// the validate_project_root guard on the backend.
{
  let called = false;
  const restore = __setPersistDisabledForTests(async () => {
    called = true;
  });
  try {
    useSkillsStore.setState({ disabledByProject: new Map() });
    useSkillsStore.getState().toggle(null, "auto-cutter");
    assert.equal(called, false, "null project must not invoke persistence");
  } finally {
    __setPersistDisabledForTests(restore);
  }
}

// hydrateFromDisk replaces in-memory state without round-tripping back
// through the persistence sink — it mirrors what the file already
// holds; writing it back would be redundant traffic.
{
  let called = false;
  const restore = __setPersistDisabledForTests(async () => {
    called = true;
  });
  try {
    useSkillsStore.setState({ disabledByProject: new Map() });
    useSkillsStore.getState().hydrateFromDisk("/proj-y", ["skill-from-disk"]);
    assert.equal(called, false, "hydrate must not write back to disk");
    assert.equal(
      useSkillsStore.getState().isDisabled("/proj-y", "skill-from-disk"),
      true,
    );
  } finally {
    __setPersistDisabledForTests(restore);
  }
}

console.log("skills-store: OK");
