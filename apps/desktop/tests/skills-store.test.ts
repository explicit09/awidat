/**
 * Pure-logic tests for the per-project skills store.
 * Exercises the helpers without spinning up a Zustand store or DOM.
 *
 * Wave 4 T1 added the on-disk persistence — write-on-toggle flushes
 * the project's disabled set to `<project>/.montage/skills.json` via a
 * Tauri command. Wave 5 B2 extended the same file to carry version +
 * provenance pins. The store exposes `__setPersistDisabledForTests`
 * and `__setPersistSkillConfigForTests` seams so we can verify the
 * invocations without mocking the IPC bridge.
 */
import { strict as assert } from "node:assert";
import {
  __setPersistDisabledForTests,
  __setPersistSkillConfigForTests,
  applyClearPin,
  applyHydrate,
  applyHydratePins,
  applySetDisabled,
  applySetPin,
  applyToggle,
  computeGetPin,
  computeIsDisabled,
  deserialize,
  shouldApplySkillHydration,
  serialize,
  useSkillsStore,
  type PinnedSkill,
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
  assert.equal(restored.disabled.get("/p1")?.has("skill-a"), true);
  assert.equal(restored.disabled.get("/p1")?.has("skill-b"), true);
  assert.equal(restored.disabled.get("/p2")?.has("skill-a"), true);
  assert.equal(restored.disabled.get("/p2")?.has("skill-b"), false);
}

// Deserialize is defensive about missing / malformed input.
{
  assert.equal(deserialize(undefined).disabled.size, 0);
  assert.equal(deserialize(undefined).pinned.size, 0);
  // Empty arrays don't materialize as empty sets.
  assert.equal(deserialize({ disabled: { "/p": [] } }).disabled.size, 0);
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
    useSkillsStore.setState({
      disabledByProject: new Map(),
      pinnedByProject: new Map(),
    });
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
    useSkillsStore.setState({
      disabledByProject: new Map(),
      pinnedByProject: new Map(),
    });
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
    useSkillsStore.setState({
      disabledByProject: new Map(),
      pinnedByProject: new Map(),
    });
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
    useSkillsStore.setState({
      disabledByProject: new Map(),
      pinnedByProject: new Map(),
    });
    useSkillsStore.getState().hydrateFromDisk("/proj-y", ["skill-from-disk"], []);
    assert.equal(called, false, "hydrate must not write back to disk");
    assert.equal(
      useSkillsStore.getState().isDisabled("/proj-y", "skill-from-disk"),
      true,
    );
  } finally {
    __setPersistDisabledForTests(restore);
  }
}

// ---------- Wave 5 B2 — pin store coverage ----------

// applySetPin / applyClearPin happy paths — single skill round-trip.
{
  let state = new Map<string, PinnedSkill[]>();
  state = applySetPin(state, "/p", { name: "auto-cutter", version: "1.0.0" });
  assert.equal(
    computeGetPin(state, "/p", "auto-cutter")?.version,
    "1.0.0",
    "pin should be retrievable after set",
  );
  state = applyClearPin(state, "/p", "auto-cutter");
  assert.equal(state.has("/p"), false, "clearing last pin drops the key");
}

// Replacing a pin for the same skill name keeps a single entry.
{
  let state = new Map<string, PinnedSkill[]>();
  state = applySetPin(state, "/p", { name: "auto-cutter", version: "1.0.0" });
  state = applySetPin(state, "/p", {
    name: "auto-cutter",
    version: "1.1.0",
    provenance: "project",
  });
  const list = state.get("/p") ?? [];
  assert.equal(list.length, 1, "duplicate names dedupe to one entry");
  assert.equal(list[0]?.version, "1.1.0");
  assert.equal(list[0]?.provenance, "project");
}

// Per-project isolation for pins.
{
  let state = new Map<string, PinnedSkill[]>();
  state = applySetPin(state, "/a", { name: "x", version: "1.0.0" });
  assert.equal(computeGetPin(state, "/a", "x")?.version, "1.0.0");
  assert.equal(computeGetPin(state, "/b", "x"), undefined);
}

// applyHydratePins replaces the pin list wholesale.
{
  let state = new Map<string, PinnedSkill[]>();
  state = applySetPin(state, "/p", { name: "stale", version: "1.0.0" });
  state = applyHydratePins(state, "/p", [
    { name: "fresh", version: "2.0.0" },
  ]);
  const list = state.get("/p") ?? [];
  assert.equal(list.length, 1);
  assert.equal(list[0]?.name, "fresh");
  // Empty list drops the key.
  state = applyHydratePins(state, "/p", []);
  assert.equal(state.has("/p"), false);
}

// serialize / deserialize round-trips pins (sorted by name).
{
  const disabledMap = new Map<string, Set<string>>();
  let pinnedMap = new Map<string, PinnedSkill[]>();
  pinnedMap = applySetPin(pinnedMap, "/p", {
    name: "zeta",
    version: "1.0.0",
  });
  pinnedMap = applySetPin(pinnedMap, "/p", {
    name: "alpha",
    provenance: "project",
  });
  const shape = serialize(disabledMap, pinnedMap);
  // Pins emerge name-sorted (applySetPin sorts on insert).
  assert.deepEqual(
    (shape.pinned?.["/p"] ?? []).map((p) => p.name),
    ["alpha", "zeta"],
  );

  const restored = deserialize(shape);
  assert.equal(restored.pinned.get("/p")?.length, 2);
  assert.equal(restored.pinned.get("/p")?.[0]?.name, "alpha");
  assert.equal(restored.pinned.get("/p")?.[1]?.name, "zeta");
}

// Store-level: setPin persists the full skill config (disabled +
// pinned), so existing disabled state survives a pin write.
{
  type Call = { projectRoot: string; disabled: string[]; pinned: PinnedSkill[] };
  const calls: Call[] = [];
  const restore = __setPersistSkillConfigForTests(
    async (projectRoot, disabled, pinned) => {
      calls.push({
        projectRoot,
        disabled: [...disabled],
        pinned: pinned.map((p) => ({ ...p })),
      });
    },
  );
  try {
    useSkillsStore.setState({
      disabledByProject: new Map([["/proj-a", new Set(["a-disabled"])]]),
      pinnedByProject: new Map(),
    });
    useSkillsStore.getState().setPin("/proj-a", {
      name: "auto-cutter",
      version: "1.0.0",
    });
    assert.equal(calls.length, 1, "setPin should persist");
    assert.deepEqual(
      calls[0]?.disabled,
      ["a-disabled"],
      "disabled list must round-trip through the pin write",
    );
    assert.deepEqual(calls[0]?.pinned, [
      { name: "auto-cutter", version: "1.0.0" },
    ]);
    // Pin readback works through the store API.
    assert.equal(
      useSkillsStore.getState().getPin("/proj-a", "auto-cutter")?.version,
      "1.0.0",
    );

    // clearPin removes it again + persists empty pin list.
    useSkillsStore.getState().clearPin("/proj-a", "auto-cutter");
    assert.equal(calls.length, 2);
    assert.deepEqual(calls[1]?.pinned, []);
    assert.equal(
      useSkillsStore.getState().getPin("/proj-a", "auto-cutter"),
      undefined,
    );
  } finally {
    __setPersistSkillConfigForTests(restore);
  }
}

// setPin on a null project root must NOT trigger persistence — same
// rationale as toggle: the global bucket has no on-disk counterpart.
{
  let called = false;
  const restore = __setPersistSkillConfigForTests(async () => {
    called = true;
  });
  try {
    useSkillsStore.setState({
      disabledByProject: new Map(),
      pinnedByProject: new Map(),
    });
    useSkillsStore.getState().setPin(null, {
      name: "auto-cutter",
      version: "1.0.0",
    });
    assert.equal(called, false, "null project must not invoke pin persistence");
    // Still cached in memory.
    assert.equal(
      useSkillsStore.getState().getPin(null, "auto-cutter")?.version,
      "1.0.0",
    );
  } finally {
    __setPersistSkillConfigForTests(restore);
  }
}

// hydrateFromDisk now carries pin data too — replaces both sides
// wholesale without round-tripping back through either sink.
{
  let disabledCalls = 0;
  let pinCalls = 0;
  const restoreDisabled = __setPersistDisabledForTests(async () => {
    disabledCalls += 1;
  });
  const restorePins = __setPersistSkillConfigForTests(async () => {
    pinCalls += 1;
  });
  try {
    useSkillsStore.setState({
      disabledByProject: new Map(),
      pinnedByProject: new Map(),
    });
    useSkillsStore
      .getState()
      .hydrateFromDisk(
        "/proj-z",
        ["disabled-skill"],
        [{ name: "pinned-skill", version: "1.0.0", provenance: "user" }],
      );
    assert.equal(disabledCalls, 0, "hydrate must not invoke disabled persist");
    assert.equal(pinCalls, 0, "hydrate must not invoke pin persist");
    assert.equal(
      useSkillsStore.getState().isDisabled("/proj-z", "disabled-skill"),
      true,
    );
    assert.equal(
      useSkillsStore.getState().getPin("/proj-z", "pinned-skill")?.provenance,
      "user",
    );
  } finally {
    __setPersistDisabledForTests(restoreDisabled);
    __setPersistSkillConfigForTests(restorePins);
  }
}

// Deferred disk hydration must not clobber local edits made after the
// hydrate read was scheduled.
{
  assert.equal(shouldApplySkillHydration(0, 0), true);
  assert.equal(shouldApplySkillHydration(0, 1), false);
}

console.log("skills-store: OK");
