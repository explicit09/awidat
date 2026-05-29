// Tests for useIntroState — the persisted set of projects that have
// already received their synthetic editorial intro turn (Wave 3 F3).
//
// The store itself is the single decision-point for whether the intro
// fires. Conditions on the agent/media side (running flag, source
// count, composer emptiness) live in `appGlue.ts`; this test pins the
// store-level contract: persistence round-trips, marking is idempotent,
// reset wipes the set, and the sentinel detector recognises the
// intro-prefixed string.

import { strict as assert } from "node:assert";
import {
  createIntroStateStore,
  INTRO_PROMPT,
  INTRO_PROMPT_PREFIX,
  isIntroSyntheticInput,
  STORAGE_KEY,
} from "../src/state/introState.ts";

// fresh store starts empty
{
  const store = createIntroStateStore({ persist: false });
  assert.equal(store.getState().introduced.size, 0);
  assert.equal(store.getState().hasIntroduced("/some/path"), false);
}

// markIntroduced records the path
{
  const store = createIntroStateStore({ persist: false });
  store.getState().markIntroduced("/projects/foo");
  assert.equal(store.getState().hasIntroduced("/projects/foo"), true);
  assert.equal(store.getState().introduced.size, 1);
}

// markIntroduced is idempotent — second mark is a no-op
{
  const store = createIntroStateStore({ persist: false });
  store.getState().markIntroduced("/projects/foo");
  const firstRef = store.getState().introduced;
  store.getState().markIntroduced("/projects/foo");
  assert.equal(store.getState().introduced.size, 1);
  // The set reference shouldn't change either — locks the contract
  // that re-marking won't trigger spurious subscriber notifications.
  assert.equal(store.getState().introduced, firstRef);
}

// reset wipes the set and removes the persisted entry
{
  const fake: Record<string, string> = {};
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: (k: string, v: string) => { fake[k] = v; },
    removeItem: (k: string) => { delete fake[k]; },
  };
  const store = createIntroStateStore({ persist: true, storage });
  store.getState().markIntroduced("/projects/foo");
  store.getState().markIntroduced("/projects/bar");
  assert.equal(store.getState().introduced.size, 2);
  assert.ok(fake[STORAGE_KEY]);
  store.getState().reset();
  assert.equal(store.getState().introduced.size, 0);
  assert.equal(fake[STORAGE_KEY], undefined);
}

// persistence round-trips through the storage adapter — a fresh store
// reads the previously-marked path back. This is the "intro only fires
// once per project ever" guarantee from a state-machine perspective:
// re-opening the app must not re-fire on a project we've already
// introduced.
{
  const fake: Record<string, string> = {};
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: (k: string, v: string) => { fake[k] = v; },
    removeItem: (k: string) => { delete fake[k]; },
  };
  const a = createIntroStateStore({ persist: true, storage });
  a.getState().markIntroduced("/projects/foo");
  // Simulate process restart — new store reads from same storage.
  const b = createIntroStateStore({ persist: true, storage });
  assert.equal(b.getState().hasIntroduced("/projects/foo"), true);
  assert.equal(b.getState().introduced.size, 1);
}

// corrupt persisted entry falls through to the empty set
{
  const fake: Record<string, string> = { [STORAGE_KEY]: "not-json-{[" };
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: (k: string, v: string) => { fake[k] = v; },
    removeItem: (k: string) => { delete fake[k]; },
  };
  const store = createIntroStateStore({ persist: true, storage });
  assert.equal(store.getState().introduced.size, 0);
}

// non-array persisted entry also falls through
{
  const fake: Record<string, string> = { [STORAGE_KEY]: '{"foo": 1}' };
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: () => {},
    removeItem: () => {},
  };
  const store = createIntroStateStore({ persist: true, storage });
  assert.equal(store.getState().introduced.size, 0);
}

// Two-project flow mirrors the integration scenario: open A → intro
// fires (marked), open B → intro fires for B (marked, A still marked),
// reopen A → no intro (would-be-fired guard sees marked).
{
  const store = createIntroStateStore({ persist: false });
  // First open of /a — intro hasn't fired yet.
  assert.equal(store.getState().hasIntroduced("/a"), false);
  store.getState().markIntroduced("/a");
  // First open of /b — also hasn't fired yet (independent path).
  assert.equal(store.getState().hasIntroduced("/b"), false);
  store.getState().markIntroduced("/b");
  // Reopen /a — already-marked, would NOT fire again.
  assert.equal(store.getState().hasIntroduced("/a"), true);
  assert.equal(store.getState().introduced.size, 2);
}

// INTRO_PROMPT carries the sentinel prefix — the dispatcher in appGlue
// passes this exact string to start_turn; the renderer detects the
// prefix and hides the resulting UserInput item.
{
  assert.ok(
    INTRO_PROMPT.startsWith(INTRO_PROMPT_PREFIX),
    "INTRO_PROMPT must start with INTRO_PROMPT_PREFIX so the chat renderer can detect it",
  );
}

// isIntroSyntheticInput recognises the synthetic prompt
{
  assert.equal(isIntroSyntheticInput(INTRO_PROMPT), true);
  assert.equal(isIntroSyntheticInput(`${INTRO_PROMPT_PREFIX} anything goes here`), true);
}

// Normal user input does NOT match the sentinel — the prefix is unique
// enough that any user typing it on purpose is a deliberate opt-in.
{
  assert.equal(isIntroSyntheticInput("Cut the silence at 0:12"), false);
  assert.equal(isIntroSyntheticInput(""), false);
  assert.equal(isIntroSyntheticInput("intro: please introduce yourself"), false);
}

console.log("intro-state: OK");
