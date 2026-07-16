import assert from "node:assert/strict";
import {
  ABSOLUTE_TIMEOUT_MS,
  NO_PROGRESS_TIMEOUT_MS,
  checkWatchdog,
  initWatchdog,
  recordProgress,
} from "./exportJobWatchdog.ts";

// A healthy job: progress advances every tick, well under both caps.
// Never trips.
{
  const start = 1_000_000;
  let state = initWatchdog(start);
  let now = start;
  for (let i = 1; i <= 10; i++) {
    now += 30_000; // 30s ticks
    state = recordProgress(state, { state: "running", progressPct: i * 10, timeDoneS: i }, now);
    const verdict = checkWatchdog(state, now);
    assert.equal(verdict.tripped, false, `tick ${i}: steady progress must not trip the watchdog`);
  }
  console.log("ok  steady progress never trips");
}

// Stuck job: state flips to "running" once, then progress never
// changes again. Must trip once NO_PROGRESS_TIMEOUT_MS has elapsed
// since the last observed change, well before the absolute cap.
{
  const start = 2_000_000;
  let state = initWatchdog(start);
  const stuckSnapshot = { state: "running" as const, progressPct: 5, timeDoneS: 1.2 };

  // First tick establishes the snapshot.
  let now = start + 1_000;
  state = recordProgress(state, stuckSnapshot, now);
  assert.equal(checkWatchdog(state, now).tripped, false, "first running tick must not trip");

  // Repeat the identical snapshot every 30s; nothing changes.
  let trippedAt: number | null = null;
  for (let i = 0; i < 20; i++) {
    now += 30_000;
    state = recordProgress(state, stuckSnapshot, now);
    const verdict = checkWatchdog(state, now);
    if (verdict.tripped) {
      trippedAt = now;
      assert.match(
        verdict.reason,
        /no progress/i,
        "no-progress trip reason should name the cause",
      );
      break;
    }
  }
  assert.ok(trippedAt !== null, "watchdog must eventually trip on a stuck job");
  const elapsedSinceStuck = (trippedAt as number) - (start + 1_000);
  assert.ok(
    elapsedSinceStuck >= NO_PROGRESS_TIMEOUT_MS,
    `should not trip before the no-progress window elapses (elapsed=${elapsedSinceStuck})`,
  );
  assert.ok(
    elapsedSinceStuck < ABSOLUTE_TIMEOUT_MS,
    "no-progress trip should fire well before the absolute cap",
  );
  console.log("ok  stuck-but-running job trips the no-progress watchdog before the absolute cap");
}

// Queued forever (never reaches "running"): the no-progress condition
// is scoped to "running" only, so a job stuck in "queued" relies on
// the absolute cap instead.
{
  const start = 3_000_000;
  let state = initWatchdog(start);
  let now = start;
  for (let i = 0; i < 6; i++) {
    now += 60_000; // 1 min ticks, well past NO_PROGRESS_TIMEOUT_MS
    state = recordProgress(state, { state: "queued", progressPct: null, timeDoneS: null }, now);
    assert.equal(
      checkWatchdog(state, now).tripped,
      false,
      "queued-only state must not trip the no-progress condition",
    );
  }
  now = start + ABSOLUTE_TIMEOUT_MS;
  const verdict = checkWatchdog(state, now);
  assert.equal(verdict.tripped, true, "absolute cap must still fire for a job stuck in queued");
  assert.match(verdict.reason, /timed out/i);
  console.log("ok  queued-forever job is caught by the absolute cap, not the no-progress one");
}

// Absolute cap fires exactly at the boundary even with progress ticking
// (defends against a job that keeps nudging progress just enough to
// dodge the no-progress window but never actually finishes).
{
  const start = 4_000_000;
  let state = initWatchdog(start);
  let now = start;
  for (let i = 1; i <= 100; i++) {
    now += 20_000; // 20s ticks with strictly increasing progress
    state = recordProgress(
      state,
      { state: "running", progressPct: null, timeDoneS: i * 0.001 },
      now,
    );
    const verdict = checkWatchdog(state, now);
    if (now - start >= ABSOLUTE_TIMEOUT_MS) {
      assert.equal(verdict.tripped, true, "absolute cap must fire even with nudging progress");
      assert.match(verdict.reason, /timed out/i);
      console.log("ok  absolute cap fires even when progress keeps nudging");
      break;
    }
    assert.equal(verdict.tripped, false, `tick ${i}: before the absolute cap, must not trip`);
  }
}

console.log("export-job-watchdog: OK");
