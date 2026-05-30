// Tests for the W5.A2 upload-state extensions to the render queue
// store. We don't mount React — we exercise the store actions
// directly (the same way `mode-store.test.ts` does) and assert the
// resulting entry shape matches the wire contract.

import { strict as assert } from "node:assert";

// localStorage isn't available in node by default; the store handles
// that via a typeof check, so we don't shim it here.
import {
  newQueueId,
  useRenderQueueStore,
  type RenderQueueEntry,
  type RenderUploadState,
} from "../src/app/renderQueue.ts";

function resetStore(): void {
  // The store persists to localStorage when available — in node it
  // doesn't, but we still want a clean per-test entries array.
  useRenderQueueStore.setState({ entries: [] });
}

function sampleEntry(overrides: Partial<RenderQueueEntry> = {}): RenderQueueEntry {
  return {
    id: newQueueId("master"),
    targetId: "youtube",
    label: "YouTube 1080p",
    kind: "video_master",
    status: "pending",
    enqueuedAt: Date.now(),
    ...overrides,
  };
}

// ---- setUploadTargets seeds Pending state per provider ----
{
  resetStore();
  const entry = sampleEntry();
  useRenderQueueStore.getState().enqueue([entry]);
  useRenderQueueStore
    .getState()
    .setUploadTargets(entry.id, ["youtube", "tiktok"]);
  const stored = useRenderQueueStore
    .getState()
    .entries.find((e) => e.id === entry.id);
  assert.ok(stored, "entry persisted");
  assert.deepEqual(stored?.uploadTargets, ["youtube", "tiktok"]);
  assert.deepEqual(stored?.uploadStates, {
    youtube: { state: "pending" },
    tiktok: { state: "pending" },
  });
  assert.deepEqual(stored?.publishedUrls, {});
}

// ---- setUploadStates round-trips the wire shape ----
{
  resetStore();
  const entry = sampleEntry();
  useRenderQueueStore.getState().enqueue([entry]);
  useRenderQueueStore
    .getState()
    .setUploadTargets(entry.id, ["youtube"]);

  // Backend reports "uploading 30%"
  const states1: Record<string, RenderUploadState> = {
    youtube: { state: "uploading", progress: 0.3 },
  };
  useRenderQueueStore.getState().setUploadStates(entry.id, states1, {});
  const mid = useRenderQueueStore
    .getState()
    .entries.find((e) => e.id === entry.id);
  assert.deepEqual(mid?.uploadStates?.youtube, {
    state: "uploading",
    progress: 0.3,
  });

  // Backend reports "published"
  const states2: Record<string, RenderUploadState> = {
    youtube: {
      state: "published",
      remote_url: "https://youtu.be/abc",
      remote_id: "abc",
    },
  };
  const urls = { youtube: "https://youtu.be/abc" };
  useRenderQueueStore.getState().setUploadStates(entry.id, states2, urls);
  const done = useRenderQueueStore
    .getState()
    .entries.find((e) => e.id === entry.id);
  assert.equal(done?.uploadStates?.youtube?.state, "published");
  if (done?.uploadStates?.youtube?.state === "published") {
    assert.equal(done.uploadStates.youtube.remote_url, "https://youtu.be/abc");
  }
  assert.equal(done?.publishedUrls?.youtube, "https://youtu.be/abc");
}

// ---- failed upload state stays terminal ----
{
  resetStore();
  const entry = sampleEntry();
  useRenderQueueStore.getState().enqueue([entry]);
  useRenderQueueStore
    .getState()
    .setUploadTargets(entry.id, ["youtube"]);
  const states: Record<string, RenderUploadState> = {
    youtube: { state: "failed", reason: "not_configured" },
  };
  useRenderQueueStore.getState().setUploadStates(entry.id, states, {});
  const stored = useRenderQueueStore
    .getState()
    .entries.find((e) => e.id === entry.id);
  assert.equal(stored?.uploadStates?.youtube?.state, "failed");
  if (stored?.uploadStates?.youtube?.state === "failed") {
    assert.equal(stored.uploadStates.youtube.reason, "not_configured");
  }
}

// ---- one target's terminal state is independent of another's ----
{
  resetStore();
  const entry = sampleEntry();
  useRenderQueueStore.getState().enqueue([entry]);
  useRenderQueueStore
    .getState()
    .setUploadTargets(entry.id, ["youtube", "tiktok"]);
  const states: Record<string, RenderUploadState> = {
    youtube: {
      state: "published",
      remote_url: "https://youtu.be/y",
      remote_id: "y",
    },
    tiktok: { state: "failed", reason: "rate_limited" },
  };
  const urls = { youtube: "https://youtu.be/y" };
  useRenderQueueStore.getState().setUploadStates(entry.id, states, urls);
  const stored = useRenderQueueStore
    .getState()
    .entries.find((e) => e.id === entry.id);
  assert.equal(stored?.uploadStates?.youtube?.state, "published");
  assert.equal(stored?.uploadStates?.tiktok?.state, "failed");
  // publishedUrls reflects only successful targets.
  assert.equal(stored?.publishedUrls?.youtube, "https://youtu.be/y");
  assert.equal(stored?.publishedUrls?.tiktok, undefined);
}

// ---- entries without uploadTargets remain unaffected ----
{
  resetStore();
  const noUploadEntry = sampleEntry({ kind: "captions", label: "Captions" });
  useRenderQueueStore.getState().enqueue([noUploadEntry]);
  // Mark done without touching upload helpers.
  useRenderQueueStore.getState().markDone(noUploadEntry.id, "/tmp/x.srt");
  const stored = useRenderQueueStore
    .getState()
    .entries.find((e) => e.id === noUploadEntry.id);
  assert.equal(stored?.status, "done");
  assert.equal(stored?.uploadTargets, undefined);
  assert.equal(stored?.uploadStates, undefined);
}

// ---- replacing upload targets resets state to Pending ----
{
  resetStore();
  const entry = sampleEntry();
  useRenderQueueStore.getState().enqueue([entry]);
  useRenderQueueStore
    .getState()
    .setUploadTargets(entry.id, ["youtube"]);
  // Imagine the user toggles tiktok on after the fact.
  useRenderQueueStore
    .getState()
    .setUploadTargets(entry.id, ["youtube", "tiktok"]);
  const stored = useRenderQueueStore
    .getState()
    .entries.find((e) => e.id === entry.id);
  assert.deepEqual(stored?.uploadTargets, ["youtube", "tiktok"]);
  assert.equal(stored?.uploadStates?.youtube?.state, "pending");
  assert.equal(stored?.uploadStates?.tiktok?.state, "pending");
  assert.deepEqual(stored?.publishedUrls, {});
}

console.log("render-queue-upload: OK");
