import { strict as assert } from "node:assert";
import { mockIPC } from "@tauri-apps/api/mocks";

(globalThis as typeof globalThis & { window: typeof globalThis }).window = globalThis;

let thumbnailReads = 0;
let waveformReads = 0;
mockIPC((command) => {
  if (command === "list_thumbnail_frames") {
    thumbnailReads += 1;
    return ["/project/.montage/thumbnails/frame-0001.jpg"];
  }
  if (command === "read_waveform") {
    waveformReads += 1;
    return { buckets: [0.25, 0.75], duration_s: 2 };
  }
  return null;
});

globalThis.requestAnimationFrame = ((callback: FrameRequestCallback) => {
  callback(0);
  return 1;
}) as typeof requestAnimationFrame;

const [{ useTranscriptStore }, thumbnailCache, waveformCache, lutCache, lifecycle] =
  await Promise.all([
    import("../src/transcript/store.ts"),
    import("../src/timeline/thumbnailCache.ts"),
    import("../src/timeline/waveformCache.ts"),
    import("../src/media/previewLutCache.ts").catch(() => null),
    import("../src/state/projectCacheLifecycle.ts").catch(() => null),
  ]);

assert.ok(lutCache, "preview LUT cache module must exist");
assert.ok(lifecycle, "project cache lifecycle module must exist");

useTranscriptStore.setState({
  byStem: {
    clip: {
      state: "loaded",
      transcript: {
        asset_stem: "clip",
        language: "en",
        diarized: false,
        segments: [],
        words: [],
        speakers: [],
      },
    },
  },
  activeStem: "clip",
  selection: { stem: "clip", startWordIdx: 0, endWordIdx: 0 },
});

thumbnailCache.getStrip("/project/.montage/thumbnails");
waveformCache.getWaveform("/project/.montage/waveform.json");
await Promise.resolve();
await Promise.resolve();

let lutLoads = 0;
await lutCache.fetchPreviewLut("/project", "look.cube", async () => {
  lutLoads += 1;
  return { size: 1, domainMin: [0, 0, 0], domainMax: [1, 1, 1], rgba: new Uint8Array(4) };
});

lifecycle.clearProjectScopedFrontendState();

const transcript = useTranscriptStore.getState();
assert.deepEqual(transcript.byStem, {}, "project switch must release loaded transcripts");
assert.equal(transcript.activeStem, null, "project switch must release active transcript identity");
assert.equal(transcript.selection, null, "project switch must release transcript selection");

thumbnailCache.getStrip("/project/.montage/thumbnails");
waveformCache.getWaveform("/project/.montage/waveform.json");
await Promise.resolve();
await Promise.resolve();
await lutCache.fetchPreviewLut("/project", "look.cube", async () => {
  lutLoads += 1;
  return { size: 1, domainMin: [0, 0, 0], domainMax: [1, 1, 1], rgba: new Uint8Array(4) };
});

assert.equal(thumbnailReads, 2, "project switch must evict decoded filmstrip state");
assert.equal(waveformReads, 2, "project switch must evict decoded waveform state");
assert.equal(lutLoads, 2, "project switch must evict parsed LUT state");

console.log("project-cache-lifecycle: all assertions passed");
