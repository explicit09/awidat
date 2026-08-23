export type LatestPreviewQueue<T> = {
  request: (value: T) => void;
  reset: () => void;
  dispose: () => void;
};

export function createLatestPreviewQueue<T, R>(
  run: (value: T) => Promise<R>,
  onResult: (result: R) => void,
  onError?: (error: unknown) => void,
): LatestPreviewQueue<T> {
  let latestRequestId = 0;
  let queued: { id: number; value: T } | null = null;
  let running = false;
  let disposed = false;

  async function drain(): Promise<void> {
    if (disposed || running || queued === null) return;
    const request = queued;
    queued = null;
    running = true;
    try {
      const result = await run(request.value);
      if (!disposed && request.id === latestRequestId) onResult(result);
    } catch (error) {
      if (!disposed && request.id === latestRequestId) onError?.(error);
    } finally {
      running = false;
      if (queued !== null) void drain();
    }
  }

  return {
    request(value) {
      if (disposed) return;
      queued = { id: ++latestRequestId, value };
      void drain();
    },
    reset() {
      latestRequestId += 1;
      queued = null;
    },
    dispose() {
      disposed = true;
      latestRequestId += 1;
      queued = null;
    },
  };
}
