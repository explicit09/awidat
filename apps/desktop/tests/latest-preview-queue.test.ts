import { strict as assert } from "node:assert";

const queueModule = await import("../src/media/latestPreviewQueue.ts").catch(() => null);
assert.ok(queueModule, "latest-only preview queue must exist");

type Deferred<T> = { promise: Promise<T>; resolve: (value: T) => void };
function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

const first = deferred<string>();
const latest = deferred<string>();
const started: number[] = [];
const settled: string[] = [];
const queue = queueModule.createLatestPreviewQueue<number, string>(
  async (value) => {
    started.push(value);
    return value === 1 ? first.promise : latest.promise;
  },
  (value) => settled.push(value),
);

queue.request(1);
queue.request(2);
queue.request(3);
await Promise.resolve();
assert.deepEqual(started, [1], "only one expensive preview may be in flight");

first.resolve("stale");
await new Promise((resolve) => setTimeout(resolve, 0));
assert.deepEqual(started, [1, 3], "queued work must collapse to the latest request");
assert.deepEqual(settled, [], "stale in-flight result must not reach the UI");

latest.resolve("latest");
await new Promise((resolve) => setTimeout(resolve, 0));
assert.deepEqual(settled, ["latest"]);

const abandoned = deferred<string>();
const afterReset: string[] = [];
const resettable = queueModule.createLatestPreviewQueue<number, string>(
  async () => abandoned.promise,
  (value) => afterReset.push(value),
);
resettable.request(1);
resettable.reset();
abandoned.resolve("abandoned");
await new Promise((resolve) => setTimeout(resolve, 0));
assert.deepEqual(afterReset, [], "reset must invalidate a preview that settles after its window closes");

queue.dispose();
queue.request(4);
await Promise.resolve();
assert.deepEqual(started, [1, 3], "disposed queue must not start new preview work");

console.log("latest-preview-queue: all assertions passed");
