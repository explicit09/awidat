/**
 * Pure-logic tests for the per-project indexer overlay store.
 * Exercises the helpers without spinning up a Zustand store or DOM.
 *
 * Mirrors `skills-store.test.ts` (Wave 4 T1) — the persistence sink is
 * swapped via `__setPersistDisabledForTests` so we can verify the
 * write-on-toggle invocation without mocking the IPC bridge. The
 * dispatcher reads the same file via the Rust side; that path has its
 * own Rust unit tests in `commands/indexer_config_overlay.rs`.
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
  useIndexerOverlay,
} from "../src/state/indexerOverlay.ts";

// Defaults: nothing disabled.
{
  const empty = new Map<string, Set<string>>();
  assert.equal(computeIsDisabled(empty, "/p", "face"), false);
  assert.equal(computeIsDisabled(empty, null, "face"), false);
}

// Toggle on then off — symmetric.
{
  let state = new Map<string, Set<string>>();
  state = applyToggle(state, "/p", "face");
  assert.equal(computeIsDisabled(state, "/p", "face"), true);
  state = applyToggle(state, "/p", "face");
  assert.equal(computeIsDisabled(state, "/p", "face"), false);
  // After both toggles, the project key should be gone (no leftover empty set).
  assert.equal(state.has("/p"), false);
}

// Per-project isolation — disabling in one project leaves the other alone.
{
  let state = new Map<string, Set<string>>();
  state = applyToggle(state, "/proj-a", "motion");
  assert.equal(computeIsDisabled(state, "/proj-a", "motion"), true);
  assert.equal(computeIsDisabled(state, "/proj-b", "motion"), false);
}

// setDisabled is idempotent and explicit.
{
  let state = new Map<string, Set<string>>();
  state = applySetDisabled(state, "/p", "face", true);
  state = applySetDisabled(state, "/p", "face", true);
  assert.equal(state.get("/p")?.size, 1);
  state = applySetDisabled(state, "/p", "face", false);
  assert.equal(state.has("/p"), false);
}

// serialize / deserialize is a lossless round-trip.
{
  let state = new Map<string, Set<string>>();
  state = applyToggle(state, "/p1", "face");
  state = applyToggle(state, "/p1", "motion");
  state = applyToggle(state, "/p2", "face");

  const shape = serialize(state);
  assert.deepEqual(shape.disabled["/p1"], ["face", "motion"]);
  assert.deepEqual(shape.disabled["/p2"], ["face"]);

  const restored = deserialize(shape);
  assert.equal(restored.get("/p1")?.has("face"), true);
  assert.equal(restored.get("/p1")?.has("motion"), true);
  assert.equal(restored.get("/p2")?.has("face"), true);
  assert.equal(restored.get("/p2")?.has("motion"), false);
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
  state = applyToggle(state, null, "face");
  assert.equal(computeIsDisabled(state, null, "face"), true);
  assert.equal(state.size, 1);
}

// applyHydrate replaces the project's set wholesale and clears the
// project key when the disk says "nothing disabled".
{
  let state = new Map<string, Set<string>>();
  state = applyHydrate(state, "/p", ["face", "motion"]);
  assert.equal(state.get("/p")?.has("face"), true);
  assert.equal(state.get("/p")?.has("motion"), true);
  // Re-hydrating with an empty list drops the project key.
  state = applyHydrate(state, "/p", []);
  assert.equal(state.has("/p"), false);
}

// applyHydrate replaces, does not merge. An indexer that was disabled
// in the in-memory cache but absent from disk should end up enabled
// after hydrate.
{
  let state = new Map<string, Set<string>>();
  state = applySetDisabled(state, "/p", "stale-indexer", true);
  state = applyHydrate(state, "/p", ["fresh-indexer"]);
  assert.equal(state.get("/p")?.has("stale-indexer"), false);
  assert.equal(state.get("/p")?.has("fresh-indexer"), true);
}

// Store-level: toggle on a real project root invokes the persistence
// sink with the deduped + sorted disabled list. Toggle off then writes
// the remaining single name.
{
  type Call = { projectRoot: string; disabled: string[] };
  const calls: Call[] = [];
  const restore = __setPersistDisabledForTests(async (projectRoot, disabled) => {
    calls.push({ projectRoot, disabled: [...disabled] });
  });
  try {
    // Reset the store so prior persisted localStorage state doesn't
    // bleed into our assertions.
    useIndexerOverlay.setState({ disabledByProject: new Map() });
    useIndexerOverlay.getState().toggle("/proj-a", "face");
    assert.equal(calls.length, 1, "first toggle should persist");
    assert.equal(calls[0]?.projectRoot, "/proj-a");
    assert.deepEqual(calls[0]?.disabled, ["face"]);
    useIndexerOverlay.getState().toggle("/proj-a", "motion");
    assert.equal(calls.length, 2);
    assert.deepEqual(calls[1]?.disabled, ["face", "motion"]);
    // Toggling off face persists the remaining single name.
    useIndexerOverlay.getState().toggle("/proj-a", "face");
    assert.equal(calls.length, 3);
    assert.deepEqual(calls[2]?.disabled, ["motion"]);
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
    useIndexerOverlay.setState({ disabledByProject: new Map() });
    useIndexerOverlay.getState().setDisabled("/proj-x", "face", true);
    assert.equal(calls.length, 1);
    assert.deepEqual(calls[0]?.disabled, ["face"]);
    useIndexerOverlay.getState().setDisabled("/proj-x", "face", false);
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
    useIndexerOverlay.setState({ disabledByProject: new Map() });
    useIndexerOverlay.getState().toggle(null, "face");
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
    useIndexerOverlay.setState({ disabledByProject: new Map() });
    useIndexerOverlay
      .getState()
      .hydrateFromDisk("/proj-y", ["indexer-from-disk"]);
    assert.equal(called, false, "hydrate must not write back to disk");
    assert.equal(
      useIndexerOverlay
        .getState()
        .isDisabled("/proj-y", "indexer-from-disk"),
      true,
    );
  } finally {
    __setPersistDisabledForTests(restore);
  }
}

// disabledFor returns a sorted plain array (used by tests + future UI
// surfaces that want a deterministic order).
{
  useIndexerOverlay.setState({
    disabledByProject: new Map([["/p", new Set(["motion", "face"])]]),
  });
  assert.deepEqual(useIndexerOverlay.getState().disabledFor("/p"), [
    "face",
    "motion",
  ]);
  assert.deepEqual(useIndexerOverlay.getState().disabledFor("/missing"), []);
}

console.log("indexer-overlay: OK");
