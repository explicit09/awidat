// Tests for useAgentsMdEditor — the open-state + dirty-flag store
// for the in-product AGENTS.md editor modal (Wave 3 T2).
//
// The store is the single source of truth for "is the modal open"
// and "does the buffer differ from disk". This test pins the
// invariants:
//   - fresh store is closed + clean
//   - open() flips isOpen true, leaves isDirty false
//   - markDirty() flips isDirty true (idempotent — no spurious
//     subscriber notifications on a no-op re-mark)
//   - markClean() flips it back (idempotent same way)
//   - close() resets both isOpen and isDirty so a re-open starts
//     fresh (no stale draft survives — paired with the no-persist
//     decision in the store)

import { strict as assert } from "node:assert";
import { createAgentsMdEditorStore } from "../src/state/agentsMdEditor.ts";

// fresh store starts closed and clean
{
  const store = createAgentsMdEditorStore();
  const s = store.getState();
  assert.equal(s.isOpen, false);
  assert.equal(s.isDirty, false);
}

// open() opens, dirty stays false
{
  const store = createAgentsMdEditorStore();
  store.getState().open();
  const s = store.getState();
  assert.equal(s.isOpen, true);
  assert.equal(s.isDirty, false);
}

// markDirty flips isDirty true
{
  const store = createAgentsMdEditorStore();
  store.getState().open();
  store.getState().markDirty();
  assert.equal(store.getState().isDirty, true);
}

// markDirty is idempotent — re-marking returns the same state ref
{
  const store = createAgentsMdEditorStore();
  store.getState().markDirty();
  const first = store.getState();
  store.getState().markDirty();
  const second = store.getState();
  assert.equal(second.isDirty, true);
  // The state object should be the SAME reference on the no-op
  // path — subscribers must not get spurious notifications when
  // the textarea fires onChange but the dirty flag was already set.
  assert.equal(second, first);
}

// markClean flips isDirty false
{
  const store = createAgentsMdEditorStore();
  store.getState().markDirty();
  store.getState().markClean();
  assert.equal(store.getState().isDirty, false);
}

// markClean is idempotent — calling on already-clean is a no-op
{
  const store = createAgentsMdEditorStore();
  const first = store.getState();
  store.getState().markClean();
  const second = store.getState();
  assert.equal(second.isDirty, false);
  assert.equal(second, first);
}

// close() wipes both flags so a re-open starts fresh — no stale
// draft survives an Esc cancel
{
  const store = createAgentsMdEditorStore();
  store.getState().open();
  store.getState().markDirty();
  store.getState().close();
  const s = store.getState();
  assert.equal(s.isOpen, false);
  assert.equal(s.isDirty, false);
}

// open() also wipes any leftover dirty flag from a previous session
// — defensive against a caller that forgot to call close().
{
  const store = createAgentsMdEditorStore();
  store.getState().markDirty();
  store.getState().open();
  const s = store.getState();
  assert.equal(s.isOpen, true);
  assert.equal(s.isDirty, false);
}

console.log("agents-md-editor: OK");
