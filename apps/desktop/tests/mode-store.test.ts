import { strict as assert } from "node:assert";
import { createModeStore, type Mode } from "../src/state/mode.ts";

// fresh store defaults to "pro" (existing users are editors; the spec
// defines pro as the heritage default, creator as the opinionated alt)
{
  const store = createModeStore({ persist: false });
  assert.equal(store.get(), "pro");
}

// toggle flips
{
  const store = createModeStore({ persist: false });
  store.set("creator");
  assert.equal(store.get(), "creator");
  store.toggle();
  assert.equal(store.get(), "pro");
  store.toggle();
  assert.equal(store.get(), "creator");
}

// persistence round-trips through the provided storage adapter
{
  const fake: Record<string, string> = {};
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: (k: string, v: string) => { fake[k] = v; },
    removeItem: (k: string) => { delete fake[k]; },
  };
  const a = createModeStore({ persist: true, storage });
  a.set("creator");
  // simulate process restart — new store reads from same storage
  const b = createModeStore({ persist: true, storage });
  assert.equal(b.get(), "creator");
}

// invalid persisted value falls back to default
{
  const fake: Record<string, string> = { "montage:mode": "garbage" };
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: () => {},
    removeItem: () => {},
  };
  const s = createModeStore({ persist: true, storage });
  assert.equal(s.get(), "pro");
}

// subscribe receives updates
{
  const store = createModeStore({ persist: false });
  const received: Mode[] = [];
  const unsub = store.subscribe((m) => received.push(m));
  store.set("creator");
  store.set("pro");
  unsub();
  store.set("creator");          // not received after unsubscribe
  assert.deepEqual(received, ["creator", "pro"]);
}

console.log("mode-store: OK");
