import { strict as assert } from "node:assert";
import { mockIPC } from "@tauri-apps/api/mocks";

(globalThis as typeof globalThis & { window: typeof globalThis }).window = globalThis;

type Deferred<T> = {
  promise: Promise<T>;
  resolve: (value: T) => void;
};

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

const thumbnailFirst = deferred<string[]>();
const waveformFirst = deferred<{ buckets: number[]; duration_s: number }>();
const transcriptFirst = deferred<{
  asset_stem: string;
  language: string;
  diarized: boolean;
  segments: [];
  words: [];
  speakers: [];
}>();
let thumbnailReads = 0;
let waveformReads = 0;

mockIPC((command) => {
  if (command === "list_thumbnail_frames") {
    thumbnailReads += 1;
    return thumbnailReads === 1 ? thumbnailFirst.promise : [];
  }
  if (command === "read_waveform") {
    waveformReads += 1;
    return waveformReads === 1
      ? waveformFirst.promise
      : { buckets: [], duration_s: 0 };
  }
  if (command === "read_transcript") return transcriptFirst.promise;
  return null;
});

globalThis.requestAnimationFrame = ((callback: FrameRequestCallback) => {
  callback(0);
  return 1;
}) as typeof requestAnimationFrame;

const [thumbnailCache, waveformCache, { useTranscriptStore }, lifecycle] = await Promise.all([
  import("../src/timeline/thumbnailCache.ts"),
  import("../src/timeline/waveformCache.ts"),
  import("../src/transcript/store.ts"),
  import("../src/state/projectCacheLifecycle.ts"),
]);

thumbnailCache.getStrip("/old/thumbnails");
waveformCache.getWaveform("/old/waveform.json");
const transcriptLoad = useTranscriptStore.getState().load("old-clip");
lifecycle.clearProjectScopedFrontendState();

thumbnailFirst.resolve(["/old/thumbnails/frame-0001.jpg"]);
waveformFirst.resolve({ buckets: [1], duration_s: 1 });
transcriptFirst.resolve({
  asset_stem: "old-clip",
  language: "en",
  diarized: false,
  segments: [],
  words: [],
  speakers: [],
});
await transcriptLoad;
await new Promise((resolve) => setTimeout(resolve, 0));

thumbnailCache.getStrip("/old/thumbnails");
waveformCache.getWaveform("/old/waveform.json");
await new Promise((resolve) => setTimeout(resolve, 0));

assert.equal(thumbnailReads, 2, "a stale thumbnail result must not repopulate a cleared cache");
assert.equal(waveformReads, 2, "a stale waveform result must not repopulate a cleared cache");
assert.deepEqual(
  useTranscriptStore.getState().byStem,
  {},
  "a stale transcript result must not repopulate a cleared project store",
);

console.log("project-cache-stale-requests: all assertions passed");
