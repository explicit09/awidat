// Tests for useCenterModeStore — the per-project Brief/Source/Timeline
// tab persistence (Wave 3 B1).
//
// The store has two halves:
//   1. Pure default resolution (`resolveDefaultMode`): given pending
//      count + first-session flag, pick the right mode.
//   2. Per-project persistence: explicit `set()` calls survive restart.

import { strict as assert } from "node:assert";
import {
  createCenterModeStore,
  resolveDefaultMode,
  STORAGE_KEY,
  type CenterMode,
} from "../src/state/centerMode.ts";

// resolveDefaultMode — pending proposals route to Brief
{
  assert.equal(resolveDefaultMode({ pendingCount: 3 }), "brief");
  assert.equal(resolveDefaultMode({ pendingCount: 1 }), "brief");
}

// resolveDefaultMode — first session routes to Brief even with no pending
{
  assert.equal(resolveDefaultMode({ isFirstSession: true }), "brief");
  assert.equal(
    resolveDefaultMode({ pendingCount: 0, isFirstSession: true }),
    "brief",
  );
}

// resolveDefaultMode — quiet established project lands on Source
{
  assert.equal(resolveDefaultMode({ pendingCount: 0, isFirstSession: false }), "source");
  assert.equal(resolveDefaultMode({}), "source");
  assert.equal(resolveDefaultMode(undefined), "source");
}

// Fresh store with no project routes to default resolution
{
  const store = createCenterModeStore({ persist: false });
  assert.equal(store.getState().get(null), "source");
  assert.equal(
    store.getState().get(null, { pendingCount: 2 }),
    "brief",
    "default rules apply even when project is null",
  );
}

// Per-project explicit set persists across the get() call
{
  const store = createCenterModeStore({ persist: false });
  store.getState().set("/projects/foo", "timeline");
  assert.equal(store.getState().get("/projects/foo"), "timeline");
  // The persisted choice wins over the default rule.
  assert.equal(
    store.getState().get("/projects/foo", { pendingCount: 5 }),
    "timeline",
  );
}

// Different projects keep independent state
{
  const store = createCenterModeStore({ persist: false });
  store.getState().set("/a", "brief");
  store.getState().set("/b", "timeline");
  assert.equal(store.getState().get("/a"), "brief");
  assert.equal(store.getState().get("/b"), "timeline");
  // Project with no entry falls back to default rule (no pending → source).
  assert.equal(store.getState().get("/c"), "source");
}

// set() with null project is a no-op (defends App.tsx's "no project loaded" path)
{
  const store = createCenterModeStore({ persist: false });
  store.getState().set(null, "timeline");
  assert.equal(store.getState().get("/x"), "source");
}

// set() to the same value doesn't notify (small but reasonable invariant)
{
  const store = createCenterModeStore({ persist: false });
  store.getState().set("/a", "brief");
  const before = store.getState().byProject;
  store.getState().set("/a", "brief");
  assert.equal(store.getState().byProject, before);
}

// Persistence round-trips through the storage adapter
{
  const fake: Record<string, string> = {};
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: (k: string, v: string) => { fake[k] = v; },
    removeItem: (k: string) => { delete fake[k]; },
  };
  const a = createCenterModeStore({ persist: true, storage });
  a.getState().set("/projects/foo", "timeline");
  a.getState().set("/projects/bar", "brief");
  // Simulate process restart — new store reads from same storage.
  const b = createCenterModeStore({ persist: true, storage });
  assert.equal(b.getState().get("/projects/foo"), "timeline");
  assert.equal(b.getState().get("/projects/bar"), "brief");
}

// Corrupt persisted entry falls through to empty map
{
  const fake: Record<string, string> = { [STORAGE_KEY]: "not-json-{[" };
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: () => {},
    removeItem: () => {},
  };
  const store = createCenterModeStore({ persist: true, storage });
  // Default rule applies.
  assert.equal(store.getState().get("/a"), "source");
}

// Persisted entry with invalid mode is ignored, valid neighbours survive
{
  const fake: Record<string, string> = {
    [STORAGE_KEY]: JSON.stringify({ "/a": "timeline", "/b": "garbage" }),
  };
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: () => {},
    removeItem: () => {},
  };
  const store = createCenterModeStore({ persist: true, storage });
  assert.equal(store.getState().get("/a"), "timeline");
  assert.equal(store.getState().get("/b"), "source"); // falls back to default
}

// Persisted entry with non-object root is ignored
{
  const fake: Record<string, string> = { [STORAGE_KEY]: '[1, 2, 3]' };
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: () => {},
    removeItem: () => {},
  };
  const store = createCenterModeStore({ persist: true, storage });
  assert.equal(store.getState().get("/a"), "source");
}

// Each CenterMode is a valid set target
{
  const store = createCenterModeStore({ persist: false });
  const modes: CenterMode[] = ["brief", "source", "timeline"];
  for (const mode of modes) {
    store.getState().set("/p", mode);
    assert.equal(store.getState().get("/p"), mode);
  }
}

console.log("center-mode: OK");
