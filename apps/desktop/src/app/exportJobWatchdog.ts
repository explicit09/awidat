// Client-side no-progress watchdog for `useExportJob`'s poll loop (R18).
//
// The backend's `JobManager` already bounds a render to
// `DEFAULT_JOB_TIMEOUT` (30 minutes, crates/render/src/job.rs) — but
// that bound only fires for jobs whose child process is tracked. If the
// backend job record itself gets stuck (e.g. the watch channel stops
// updating, the job manager loses track of the child, or the state
// machine wedges in "running" without the timeout task's sleep ever
// firing), `useExportJob`'s 500ms poll loop has no bound of its own: it
// only stops on `done` / `failed` / `cancelled` or a thrown `invoke`.
// Pure logic lives here (not inside the hook) so it can be tested with
// this repo's node:assert scripts instead of a React hook-testing
// harness, which this app doesn't have.
//
// Two independent trip conditions, either one ends the poll:
//   1. Absolute cap: `ABSOLUTE_TIMEOUT_MS` since the job started polling.
//      Set just past the backend's 30-minute bound so a genuinely long
//      render never trips this while the backend is still working.
//   2. No-progress cap: `NO_PROGRESS_TIMEOUT_MS` with zero observed
//      progress delta while the backend reports "running". Catches a
//      wedged job well before the 35-minute absolute cap for the common
//      case (state flips to running, then nothing ever updates again).

/** Just past the backend's `DEFAULT_JOB_TIMEOUT` (30 min) so this only
 *  fires when the backend bound itself has failed to protect us. */
export const ABSOLUTE_TIMEOUT_MS = 35 * 60 * 1000;

/** No observed progress change for this long while "running" trips the
 *  watchdog well before the absolute cap. */
export const NO_PROGRESS_TIMEOUT_MS = 5 * 60 * 1000;

/** Comparable snapshot of one poll's progress. Two snapshots are
 *  "the same" (no progress) when every field is `===` equal. */
export type ProgressSnapshot = {
  state: "queued" | "running" | "done" | "failed" | "cancelled";
  progressPct: number | null;
  timeDoneS: number | null;
};

/** Watchdog state threaded through the poll loop via a ref. */
export type WatchdogState = {
  /** `Date.now()` when polling for this job began. */
  startedAtMs: number;
  /** `Date.now()` of the last poll tick whose progress snapshot
   *  differed from the one before it. */
  lastProgressChangeAtMs: number;
  /** The most recent progress snapshot, or `null` before the first poll. */
  lastSnapshot: ProgressSnapshot | null;
};

export function initWatchdog(nowMs: number): WatchdogState {
  return { startedAtMs: nowMs, lastProgressChangeAtMs: nowMs, lastSnapshot: null };
}

function snapshotsEqual(a: ProgressSnapshot, b: ProgressSnapshot): boolean {
  return a.state === b.state && a.progressPct === b.progressPct && a.timeDoneS === b.timeDoneS;
}

/** Fold one poll tick's snapshot into watchdog state. Call this before
 *  `checkWatchdog` on every tick so the "no progress" clock only resets
 *  on an actual observed change. Returns a new state (pure — does not
 *  mutate `prev`). */
export function recordProgress(
  prev: WatchdogState,
  snapshot: ProgressSnapshot,
  nowMs: number,
): WatchdogState {
  if (prev.lastSnapshot !== null && snapshotsEqual(prev.lastSnapshot, snapshot)) {
    return { ...prev, lastSnapshot: snapshot };
  }
  return { ...prev, lastProgressChangeAtMs: nowMs, lastSnapshot: snapshot };
}

export type WatchdogResult = { tripped: false } | { tripped: true; reason: string };

/** Evaluate both trip conditions against the current state. Call after
 *  `recordProgress` on every tick. */
export function checkWatchdog(state: WatchdogState, nowMs: number): WatchdogResult {
  const sinceStartMs = nowMs - state.startedAtMs;
  if (sinceStartMs >= ABSOLUTE_TIMEOUT_MS) {
    return {
      tripped: true,
      reason: `export timed out after ${formatMinutes(ABSOLUTE_TIMEOUT_MS)} with no completion`,
    };
  }

  const isRunning = state.lastSnapshot?.state === "running";
  const sinceProgressMs = nowMs - state.lastProgressChangeAtMs;
  if (isRunning && sinceProgressMs >= NO_PROGRESS_TIMEOUT_MS) {
    return {
      tripped: true,
      reason: `export made no progress for ${formatMinutes(NO_PROGRESS_TIMEOUT_MS)}; assuming it is stuck`,
    };
  }

  return { tripped: false };
}

function formatMinutes(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  return `${minutes}m`;
}
