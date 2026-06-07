// Tests for the W5.A2 upload-state extensions to the render queue
// store. We don't mount React — we exercise the store actions
// directly (the same way `mode-store.test.ts` does) and assert the
// resulting entry shape matches the wire contract.

import { strict as assert } from "node:assert";

// localStorage isn't available in node by default; the store handles
// that via a typeof check, so we don't shim it here.
import {
  deriveUploadTargetActions,
  deriveUploadTargetRetryMode,
  hasRunnablePendingWithoutRunning,
  newQueueId,
  reframeMasterPathForEntry,
  renderQueueApprovalCopy,
  renderQueueProgressCopy,
  renderQueueStatusLabel,
  renderQueueVisibleEntries,
  serverUploadRefreshCommand,
  sourceDependencyFailure,
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

// ---- enqueue ignores duplicate active visible targets and their hidden source ----
{
  resetStore();
  const oldSource = sampleEntry({
    id: "old_source",
    label: "Source master render",
    internal: true,
  });
  const oldTikTok = sampleEntry({
    id: "old_tiktok",
    targetId: "tiktok",
    label: "TikTok",
    kind: "video_reframe",
    status: "pending",
    sourceEntryId: "old_source",
  });
  const newSource = sampleEntry({
    id: "new_source",
    label: "Source master render",
    internal: true,
  });
  const newTikTok = sampleEntry({
    id: "new_tiktok",
    targetId: "tiktok",
    label: "TikTok",
    kind: "video_reframe",
    status: "pending",
    sourceEntryId: "new_source",
  });
  useRenderQueueStore.getState().enqueue([oldSource, oldTikTok]);
  useRenderQueueStore.getState().enqueue([newSource, newTikTok]);
  assert.deepEqual(
    useRenderQueueStore.getState().entries.map((entry) => entry.id),
    ["old_source", "old_tiktok"],
    "duplicate active target and its new hidden source should be ignored instead of stacked",
  );
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

// ---- setUploadAccountIds stores concrete connected-account choices ----
{
  resetStore();
  const entry = sampleEntry();
  useRenderQueueStore.getState().enqueue([entry]);
  useRenderQueueStore
    .getState()
    .setUploadAccountIds(entry.id, {
      youtube: "acct_yt_b",
      tiktok: "acct_tt_1",
    });
  const stored = useRenderQueueStore
    .getState()
    .entries.find((e) => e.id === entry.id);
  assert.deepEqual(stored?.uploadAccountIds, {
    youtube: "acct_yt_b",
    tiktok: "acct_tt_1",
  });
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

// ---- render done + active upload reads as publishing, not complete ----
{
  const queuedUploadEntry = sampleEntry({
    status: "done",
    outputPath: "/tmp/tiktok.mp4",
    reviewStatus: "approved",
    uploadTargets: ["tiktok"],
    uploadStates: {
      tiktok: { state: "scheduled", job_id: "job_tiktok" },
    },
  });
  assert.equal(
    renderQueueStatusLabel(queuedUploadEntry),
    "Publishing",
    "a completed render with an active server publish must not show as Done",
  );
  assert.equal(
    renderQueueApprovalCopy(queuedUploadEntry),
    "Render approved. Publishing is still in progress.",
    "approval copy should not imply delivery is complete while upload is active",
  );
}

// ---- render done + failed upload reads as action needed, not complete ----
{
  const failedUploadEntry = sampleEntry({
    status: "done",
    outputPath: "/tmp/tiktok.mp4",
    reviewStatus: "approved",
    uploadTargets: ["tiktok"],
    uploadStates: {
      tiktok: {
        state: "failed",
        reason: "account_not_eligible",
        job_id: "job_tiktok",
      },
    },
  });
  assert.equal(
    renderQueueStatusLabel(failedUploadEntry),
    "Needs action",
    "a completed render with a failed server publish must not show as Done",
  );
  assert.equal(
    renderQueueApprovalCopy(failedUploadEntry),
    "Render approved. Publishing needs action.",
    "approval copy should not imply delivery is complete after upload failure",
  );
}

// ---- render done + private provider publish reads complete without URL ----
{
  const privatePublishedEntry = sampleEntry({
    status: "done",
    outputPath: "/tmp/tiktok.mp4",
    reviewStatus: "approved",
    uploadTargets: ["tiktok"],
    uploadStates: {
      tiktok: {
        state: "published",
        remote_id: "v_pub_file~private",
      },
    },
  });
  assert.equal(
    renderQueueStatusLabel(privatePublishedEntry),
    "Done",
    "a completed private provider publish without a public URL must not stay in Publishing",
  );
  assert.equal(
    renderQueueApprovalCopy(privatePublishedEntry),
    "Approved for delivery.",
    "approval copy should read complete for private provider publishes without a URL",
  );
}

// ---- internal source renders stay out of the visible user queue ----
{
  const internalMaster = sampleEntry({
    id: "internal_master",
    label: "Source master render",
    internal: true,
  });
  const tiktokEntry = sampleEntry({
    id: "tiktok_entry",
    targetId: "tiktok",
    label: "TikTok",
    kind: "video_reframe",
  });
  assert.deepEqual(
    renderQueueVisibleEntries([internalMaster, tiktokEntry]).map((entry) => entry.id),
    ["tiktok_entry"],
    "internal source renders should not appear as separate user-selected queue rows",
  );
}

// ---- visible queue collapses stale duplicate active targets ----
{
  const firstTikTok = sampleEntry({
    id: "first_tiktok",
    targetId: "tiktok",
    label: "TikTok",
    kind: "video_reframe",
    status: "pending",
  });
  const secondTikTok = sampleEntry({
    id: "second_tiktok",
    targetId: "tiktok",
    label: "TikTok",
    kind: "video_reframe",
    status: "pending",
  });
  assert.deepEqual(
    renderQueueVisibleEntries([firstTikTok, secondTikTok]).map((entry) => entry.id),
    ["first_tiktok"],
    "stale duplicate active targets should collapse to one visible row",
  );
}

// ---- visible reframe reflects hidden source-render dependency ----
{
  const internalMaster = sampleEntry({
    id: "internal_master",
    label: "Source master render",
    internal: true,
    status: "running",
    progress: 42,
  });
  const tiktokEntry = sampleEntry({
    id: "tiktok_entry",
    targetId: "tiktok",
    label: "TikTok",
    kind: "video_reframe",
    sourceEntryId: "internal_master",
  });
  assert.equal(
    renderQueueStatusLabel(tiktokEntry, [internalMaster, tiktokEntry]),
    "Preparing source",
    "TikTok should show source preparation instead of generic queued while its hidden source render runs",
  );
}

// ---- progress ticks retain ETA detail for queue copy ----
{
  resetStore();
  const entry = sampleEntry({
    id: "source_with_eta",
    label: "Source master render",
    kind: "video_master",
    status: "running",
  });
  useRenderQueueStore.getState().enqueue([entry]);
  useRenderQueueStore.getState().markProgress(entry.id, 42, {
    phase: "rendering_source",
    etaS: 75,
    timeDoneS: 32,
  });
  const stored = useRenderQueueStore
    .getState()
    .entries.find((candidate) => candidate.id === entry.id);
  assert.equal(stored?.progress, 42);
  assert.equal(stored?.progressEtaS, 75);
  assert.equal(stored?.progressTimeDoneS, 32);
  assert.equal(
    stored ? renderQueueProgressCopy(stored) : null,
    "Rendering source · 42% · ~1m 15s left",
    "running renders should show phase, percent, and ETA when the backend provides it",
  );
}

// ---- dependent TikTok row shows hidden source progress and ETA ----
{
  const internalMaster = sampleEntry({
    id: "internal_master_eta",
    label: "Source master render",
    internal: true,
    status: "running",
    progress: 31,
    progressEtaS: 90,
    progressPhase: "rendering_source",
  });
  const tiktokEntry = sampleEntry({
    id: "tiktok_waiting_on_source",
    targetId: "tiktok",
    label: "TikTok",
    kind: "video_reframe",
    sourceEntryId: "internal_master_eta",
  });
  assert.equal(
    renderQueueProgressCopy(tiktokEntry, [internalMaster, tiktokEntry]),
    "Rendering source · 31% · ~1m 30s left",
    "visible platform rows should explain hidden source-render progress",
  );
}

// ---- reframe can resume from a persisted completed source after reload ----
{
  const internalMaster = sampleEntry({
    id: "internal_master_done",
    label: "Source master render",
    internal: true,
    status: "done",
    outputPath: "/tmp/source-master.mp4",
  });
  const tiktokEntry = sampleEntry({
    id: "tiktok_entry",
    targetId: "tiktok",
    label: "TikTok",
    kind: "video_reframe",
    sourceEntryId: "internal_master_done",
  });
  assert.equal(
    reframeMasterPathForEntry(tiktokEntry, [internalMaster, tiktokEntry], null),
    "/tmp/source-master.mp4",
    "after reload, a reframe should use its completed source entry instead of requiring an in-memory lastMasterPathRef",
  );
}

// ---- reframe surfaces source failure instead of generic missing master ----
{
  const failedSource = sampleEntry({
    id: "failed_source",
    label: "Source master render",
    internal: true,
    status: "failed",
    error: "no project loaded",
  });
  const tiktokEntry = sampleEntry({
    id: "tiktok_entry",
    targetId: "tiktok",
    label: "TikTok",
    kind: "video_reframe",
    sourceEntryId: "failed_source",
  });
  assert.equal(
    sourceDependencyFailure(tiktokEntry, [failedSource, tiktokEntry]),
    "source render failed: no project loaded",
    "a dependent reframe should report the hidden source failure",
  );
}

// ---- worker can detect a stale lock with pending entries but no running work ----
{
  const pendingSource = sampleEntry({
    id: "pending_source",
    label: "Source master render",
    internal: true,
    status: "pending",
  });
  const pendingTikTok = sampleEntry({
    id: "pending_tiktok",
    targetId: "tiktok",
    label: "TikTok",
    kind: "video_reframe",
    sourceEntryId: "pending_source",
    status: "pending",
  });
  assert.equal(
    hasRunnablePendingWithoutRunning([pendingSource, pendingTikTok]),
    true,
    "pending queue entries with no running entry mean a busy worker lock is stale and should be reset",
  );
  assert.equal(
    hasRunnablePendingWithoutRunning([
      { ...pendingSource, status: "running" },
      pendingTikTok,
    ]),
    false,
    "a real running render should keep the worker lock intact",
  );
  assert.equal(
    hasRunnablePendingWithoutRunning([
      {
        ...pendingSource,
        status: "done",
        uploadTargets: ["youtube"],
        uploadStates: {
          youtube: { state: "scheduled", job_id: "job_youtube" },
        },
      },
      pendingTikTok,
    ]),
    false,
    "a completed render with active upload polling should keep the worker lock intact",
  );
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

// ---- upload target action derivation exposes durable server controls ----
{
  assert.deepEqual(
    deriveUploadTargetActions({ state: "scheduled", job_id: "job_1" }),
    {
      canRefresh: true,
      canRetry: false,
      canCancel: true,
      canReschedule: true,
      canOpenProviderUrl: false,
    },
  );
  assert.deepEqual(
    deriveUploadTargetActions({ state: "processing", job_id: "job_2" }),
    {
      canRefresh: true,
      canRetry: false,
      canCancel: true,
      canReschedule: false,
      canOpenProviderUrl: false,
    },
  );
  assert.deepEqual(
    deriveUploadTargetActions({
      state: "published",
      remote_url: "https://provider.example/post",
      remote_id: "post_1",
    }),
    {
      canRefresh: false,
      canRetry: false,
      canCancel: false,
      canReschedule: false,
      canOpenProviderUrl: true,
    },
  );
  assert.deepEqual(
    deriveUploadTargetActions({ state: "failed", reason: "rate_limited", job_id: "job_3" }),
    {
      canRefresh: true,
      canRetry: true,
      canCancel: false,
      canReschedule: false,
      canOpenProviderUrl: false,
    },
  );
}

// ---- server-backed refresh advances processing jobs through provider polling ----
{
  assert.equal(
    serverUploadRefreshCommand({ state: "scheduled", job_id: "job_sched" }),
    "social_publish_job",
  );
  assert.equal(
    serverUploadRefreshCommand({ state: "processing", job_id: "job_processing" }),
    "social_poll_publish_job",
  );
  assert.equal(
    serverUploadRefreshCommand({
      state: "failed",
      reason: "missing scope",
      job_id: "job_action",
    }),
    "social_publish_job",
  );
}

// ---- failed upload retry mode preserves server job ownership when possible ----
{
  assert.equal(
    deriveUploadTargetRetryMode(
      { state: "failed", reason: "rate_limited", job_id: "job_social_1" },
      true,
    ),
    "server_job",
  );
  assert.equal(
    deriveUploadTargetRetryMode({ state: "failed", reason: "network" }, true),
    "republish",
  );
  assert.equal(
    deriveUploadTargetRetryMode({ state: "failed", reason: "network" }, false),
    null,
  );
  assert.equal(
    deriveUploadTargetRetryMode({ state: "scheduled", job_id: "job_social_2" }, true),
    null,
  );
}

console.log("render-queue-upload: OK");
