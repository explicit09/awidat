export type DeferredHydrationScheduler = {
  setTimeout: (callback: () => void, delay?: number) => number;
  clearTimeout: (id: number) => void;
  requestIdleCallback?: (
    callback: IdleRequestCallback,
    options?: IdleRequestOptions,
  ) => number;
  cancelIdleCallback?: (id: number) => void;
};

function defaultScheduler(): DeferredHydrationScheduler {
  return window;
}

/**
 * Run non-critical app hydration after the first interactive paint.
 *
 * The timeout gives React/Tauri a chance to paint the shell first. On
 * browsers with requestIdleCallback, the actual work waits for idle
 * with a timeout so background hydration still progresses.
 */
export function deferNonCriticalHydration(
  task: () => void,
  scheduler: DeferredHydrationScheduler = defaultScheduler(),
  delayMs = 500,
): () => void {
  let cancelled = false;
  let idleId: number | null = null;

  const timeoutId = scheduler.setTimeout(() => {
    if (cancelled) return;
    if (scheduler.requestIdleCallback) {
      idleId = scheduler.requestIdleCallback(
        () => {
          if (!cancelled) task();
        },
        { timeout: 1_000 },
      );
      return;
    }
    task();
  }, delayMs);

  return () => {
    cancelled = true;
    scheduler.clearTimeout(timeoutId);
    if (idleId !== null && scheduler.cancelIdleCallback) {
      scheduler.cancelIdleCallback(idleId);
    }
  };
}
