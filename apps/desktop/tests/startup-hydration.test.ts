import { strict as assert } from "node:assert";
import {
  deferNonCriticalHydration,
  type DeferredHydrationScheduler,
} from "../src/app/startupHydration.ts";

function createScheduler(): DeferredHydrationScheduler & {
  runTimeouts: () => void;
  runIdle: () => void;
  timeoutCount: () => number;
  idleCount: () => number;
  timeoutDelays: () => Array<number | undefined>;
} {
  let nextId = 1;
  const timeouts = new Map<number, () => void>();
  const idle = new Map<number, () => void>();
  const delays: Array<number | undefined> = [];

  return {
    setTimeout(callback, delay) {
      const id = nextId++;
      delays.push(delay);
      timeouts.set(id, callback);
      return id;
    },
    clearTimeout(id) {
      timeouts.delete(id);
    },
    requestIdleCallback(callback) {
      const id = nextId++;
      idle.set(id, () =>
        callback({
          didTimeout: false,
          timeRemaining: () => 10,
        } as IdleDeadline),
      );
      return id;
    },
    cancelIdleCallback(id) {
      idle.delete(id);
    },
    runTimeouts() {
      const pending = Array.from(timeouts.entries());
      timeouts.clear();
      for (const [, callback] of pending) callback();
    },
    runIdle() {
      const pending = Array.from(idle.entries());
      idle.clear();
      for (const [, callback] of pending) callback();
    },
    timeoutCount() {
      return timeouts.size;
    },
    idleCount() {
      return idle.size;
    },
    timeoutDelays() {
      return delays;
    },
  };
}

// Non-critical hydration waits for a macrotask and then browser idle.
{
  const scheduler = createScheduler();
  let calls = 0;
  deferNonCriticalHydration(() => {
    calls += 1;
  }, scheduler);

  assert.equal(calls, 0, "hydration must not run during first render");
  assert.equal(scheduler.timeoutCount(), 1);
  assert.deepEqual(scheduler.timeoutDelays(), [500]);
  scheduler.runTimeouts();
  assert.equal(calls, 0, "idle-capable browsers should wait for idle");
  assert.equal(scheduler.idleCount(), 1);
  scheduler.runIdle();
  assert.equal(calls, 1);
}

// Cancel before the timeout fires prevents stale work.
{
  const scheduler = createScheduler();
  let calls = 0;
  const cancel = deferNonCriticalHydration(() => {
    calls += 1;
  }, scheduler);

  cancel();
  scheduler.runTimeouts();
  scheduler.runIdle();
  assert.equal(calls, 0);
}

// Cancel after timeout but before idle also prevents stale work.
{
  const scheduler = createScheduler();
  let calls = 0;
  const cancel = deferNonCriticalHydration(() => {
    calls += 1;
  }, scheduler);

  scheduler.runTimeouts();
  assert.equal(scheduler.idleCount(), 1);
  cancel();
  scheduler.runIdle();
  assert.equal(calls, 0);
}

// Without requestIdleCallback, the deferred task runs on the timeout.
{
  let timeoutCallback: (() => void) | null = null;
  const scheduler: DeferredHydrationScheduler = {
    setTimeout(callback) {
      timeoutCallback = callback;
      return 1;
    },
    clearTimeout() {},
  };
  let calls = 0;
  deferNonCriticalHydration(() => {
    calls += 1;
  }, scheduler);

  assert.equal(calls, 0);
  assert.ok(timeoutCallback);
  timeoutCallback();
  assert.equal(calls, 1);
}

console.log("startup-hydration: OK");
