// Tests for useWelcome — the persisted first-launch welcome card flag
// (Wave 3 W1).
//
// The store is the decision-point for whether the welcome modal fires
// on app boot. A single localStorage key (`montage:welcome:shown`) gates
// it; once set, the modal stays dismissed until Settings "Show welcome
// again" wipes it.

import { strict as assert } from "node:assert";
import { createWelcomeStore, STORAGE_KEY } from "../src/state/welcome.ts";

// fresh store (no persistence) starts open and not-shown
{
  const store = createWelcomeStore({ persist: false });
  const s = store.getState();
  assert.equal(s.isOpen, true);
  assert.equal(s.shown, false);
}

// dismiss() closes the modal and marks shown
{
  const store = createWelcomeStore({ persist: false });
  store.getState().dismiss();
  const s = store.getState();
  assert.equal(s.isOpen, false);
  assert.equal(s.shown, true);
}

// open() is a no-op when already open (no spurious subscriber notify)
{
  const store = createWelcomeStore({ persist: false });
  const first = store.getState();
  store.getState().open();
  const second = store.getState();
  assert.equal(second.isOpen, true);
  assert.equal(second, first);
}

// markShown() persists the shown flag without closing the modal
{
  const fake: Record<string, string> = {};
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: (k: string, v: string) => { fake[k] = v; },
    removeItem: (k: string) => { delete fake[k]; },
  };
  const store = createWelcomeStore({ persist: true, storage });
  store.getState().markShown();
  assert.equal(store.getState().shown, true);
  assert.ok(typeof fake[STORAGE_KEY] === "string" && fake[STORAGE_KEY].length > 0);
}

// dismiss() round-trips through the storage adapter — a fresh store
// reads the previously-dismissed flag back. This is the "welcome only
// fires once per machine ever" guarantee.
{
  const fake: Record<string, string> = {};
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: (k: string, v: string) => { fake[k] = v; },
    removeItem: (k: string) => { delete fake[k]; },
  };
  const a = createWelcomeStore({ persist: true, storage });
  a.getState().dismiss();
  // Simulate process restart — new store reads from same storage.
  const b = createWelcomeStore({ persist: true, storage });
  assert.equal(b.getState().shown, true);
  // Modal should NOT auto-open on a second launch.
  assert.equal(b.getState().isOpen, false);
}

// reset() wipes the persisted entry and re-opens
{
  const fake: Record<string, string> = {};
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: (k: string, v: string) => { fake[k] = v; },
    removeItem: (k: string) => { delete fake[k]; },
  };
  const store = createWelcomeStore({ persist: true, storage });
  store.getState().dismiss();
  assert.ok(fake[STORAGE_KEY]);
  assert.equal(store.getState().isOpen, false);
  store.getState().reset();
  assert.equal(store.getState().shown, false);
  assert.equal(store.getState().isOpen, true);
  assert.equal(fake[STORAGE_KEY], undefined);
}

// markShown() is idempotent — calling on an already-shown store is a
// no-op (no spurious writes through the storage adapter either).
{
  let writes = 0;
  const fake: Record<string, string> = {};
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: (k: string, v: string) => {
      fake[k] = v;
      writes += 1;
    },
    removeItem: (k: string) => { delete fake[k]; },
  };
  const store = createWelcomeStore({ persist: true, storage });
  store.getState().markShown();
  store.getState().markShown();
  assert.equal(writes, 1);
}

console.log("welcome: OK");
