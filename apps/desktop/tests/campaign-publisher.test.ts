import { strict as assert } from "node:assert";

import type { RenderQueueEntry } from "../src/app/renderQueue.ts";
import {
  createCampaignManifest,
  createPlatformVariant,
  createPublishableItem,
} from "../src/campaign/manifest.ts";
import {
  campaignUploadRequests,
  startCampaignUploads,
  type CampaignUploadRequest,
} from "../src/campaign/publisher.ts";

const entries: RenderQueueEntry[] = [
  {
    id: "render-youtube",
    targetId: "youtube",
    label: "YouTube",
    kind: "video_master",
    status: "done",
    outputPath: "/tmp/youtube.mp4",
    jobId: "job-youtube",
    enqueuedAt: 1,
  },
  {
    id: "render-tiktok",
    targetId: "tiktok",
    label: "TikTok",
    kind: "video_reframe",
    status: "done",
    outputPath: "/tmp/tiktok.mp4",
    jobId: "job-tiktok",
    enqueuedAt: 2,
  },
];

const campaign = createCampaignManifest({
  campaignId: "campaign-1",
  sourceAssetId: "/tmp/project",
  campaignType: "podcast",
  title: "Campaign",
  items: [
    createPublishableItem({
      itemId: "item-render-youtube",
      kind: "long_form",
      title: "Full episode",
      description: "Episode description",
      hashtags: ["podcast"],
      artifactPath: "/tmp/youtube.mp4",
      approvalState: "approved",
    }),
    createPublishableItem({
      itemId: "item-render-tiktok",
      kind: "short",
      title: "Short clip",
      caption: "Short caption",
      hashtags: ["ai", "startup"],
      artifactPath: "/tmp/tiktok.mp4",
      approvalState: "approved",
    }),
  ],
  platformVariants: [
    createPlatformVariant({
      variantId: "variant-youtube",
      itemId: "item-render-youtube",
      platform: "youtube",
      status: "approved",
      platformFields: { visibility: "unlisted" },
    }),
    createPlatformVariant({
      variantId: "variant-tiktok",
      itemId: "item-render-tiktok",
      platform: "tiktok",
      status: "approved",
      scheduledFor: 1_782_686_400,
      platformFields: { visibility: "public" },
    }),
    createPlatformVariant({
      variantId: "variant-instagram",
      itemId: "item-render-tiktok",
      platform: "instagram",
      status: "draft",
    }),
  ],
});

const requests = campaignUploadRequests(campaign, entries);

assert.equal(requests.length, 2);
assert.deepEqual(
  requests.map((request) => request.provider),
  ["youtube", "tiktok"],
);

const youtube = requests.find((request) => request.provider === "youtube");
assert.ok(youtube);
assert.equal(youtube.jobId, "job-youtube");
assert.equal(youtube.filePath, "/tmp/youtube.mp4");
assert.equal(youtube.metadata.title, "Full episode");
assert.equal(youtube.metadata.description, "Episode description");
assert.deepEqual(youtube.metadata.tags, ["podcast"]);
assert.equal(youtube.metadata.visibility, "unlisted");

const tiktok = requests.find((request) => request.provider === "tiktok");
assert.ok(tiktok);
assert.equal(tiktok.jobId, "job-tiktok");
assert.equal(tiktok.metadata.title, "Short clip");
assert.equal(tiktok.metadata.description, "Short caption");
assert.deepEqual(tiktok.metadata.tags, ["ai", "startup"]);
assert.equal(tiktok.metadata.visibility, "public");
assert.equal(tiktok.metadata.scheduledAt, 1_782_686_400);

// --- Behavioral tests for the upload start path -------------------------

// A single-target campaign reused across the start-path tests below.
const singleEntry: RenderQueueEntry = {
  id: "render-youtube",
  targetId: "youtube",
  label: "YouTube",
  kind: "video_master",
  status: "done",
  outputPath: "/tmp/youtube.mp4",
  jobId: "job-youtube",
  enqueuedAt: 1,
};

const singleCampaign = createCampaignManifest({
  campaignId: "campaign-2",
  sourceAssetId: "/tmp/project",
  campaignType: "podcast",
  title: "Single",
  items: [
    createPublishableItem({
      itemId: "item-render-youtube",
      kind: "long_form",
      title: "Full episode",
      hashtags: ["podcast"],
      artifactPath: "/tmp/youtube.mp4",
      approvalState: "approved",
    }),
  ],
  platformVariants: [
    createPlatformVariant({
      variantId: "variant-youtube",
      itemId: "item-render-youtube",
      platform: "youtube",
      status: "approved",
      platformFields: { visibility: "unlisted" },
    }),
  ],
});

// Mock invoke recording (command, args), returning scripted poll snapshots.
function recordingInvoke(responses: Record<string, unknown[]> = {}) {
  const calls: { command: string; args?: Record<string, unknown> }[] = [];
  const cursors: Record<string, number> = {};
  const invoke = async <T>(
    command: string,
    args?: Record<string, unknown>,
  ): Promise<T> => {
    calls.push({ command, args });
    const queue = responses[command];
    if (queue && queue.length > 0) {
      const i = cursors[command] ?? 0;
      cursors[command] = Math.min(i + 1, queue.length - 1);
      return queue[i] as T;
    }
    return undefined as T;
  };
  return { invoke, calls };
}

const noSleep = async () => {};

const publishedSnapshot = {
  job_id: "job-youtube",
  upload_targets: ["youtube"],
  upload_states: { youtube: { state: "published", remote_url: "u" } },
  published_urls: { youtube: "u" },
};

// P2: campaignUploadRequests skips a target already published for the provider.
{
  const publishedEntry: RenderQueueEntry = {
    ...singleEntry,
    uploadStates: {
      youtube: { state: "published", remote_url: "https://y", remote_id: "y1" },
    },
  };
  const skipped = campaignUploadRequests(singleCampaign, [publishedEntry]);
  assert.equal(skipped.length, 0, "already-published target is skipped");

  const uploadingEntry: RenderQueueEntry = {
    ...singleEntry,
    uploadStates: { youtube: { state: "uploading", progress: 0.5 } },
  };
  assert.equal(
    campaignUploadRequests(singleCampaign, [uploadingEntry]).length,
    1,
    "non-terminal upload state stays eligible",
  );
}

// P1: compute_ai_disclosure runs before start_uploads_for_job (default-on).
{
  const { invoke, calls } = recordingInvoke({
    poll_upload_states: [publishedSnapshot],
  });
  await startCampaignUploads(singleCampaign, [singleEntry], invoke, {
    sleep: noSleep,
  });
  const order = calls.map((c) => c.command);
  const discloseIdx = order.indexOf("compute_ai_disclosure");
  const startIdx = order.indexOf("start_uploads_for_job");
  assert.ok(discloseIdx >= 0, "compute_ai_disclosure invoked");
  assert.ok(discloseIdx < startIdx, "disclosure before start_uploads_for_job");
  assert.equal(calls[discloseIdx].args?.autoDisclose, true);
  assert.equal(calls[discloseIdx].args?.jobId, "job-youtube");
}

// autoDisclose:false is forwarded for the power-user opt-out.
{
  const { invoke, calls } = recordingInvoke({
    poll_upload_states: [publishedSnapshot],
  });
  await startCampaignUploads(singleCampaign, [singleEntry], invoke, {
    sleep: noSleep,
    autoDisclose: false,
  });
  const disclose = calls.find((c) => c.command === "compute_ai_disclosure");
  assert.equal(disclose?.args?.autoDisclose, false);
}

// P2: polls poll_upload_states until every target is terminal.
{
  const { invoke, calls } = recordingInvoke({
    poll_upload_states: [
      {
        job_id: "job-youtube",
        upload_targets: ["youtube"],
        upload_states: { youtube: { state: "uploading", progress: 0.5 } },
        published_urls: {},
      },
      publishedSnapshot,
    ],
  });
  await startCampaignUploads(singleCampaign, [singleEntry], invoke, {
    sleep: noSleep,
    maxPollTicks: 10,
  });
  const pollCount = calls.filter(
    (c) => c.command === "poll_upload_states",
  ).length;
  assert.ok(pollCount >= 2, "polled until terminal state");
}

// onStarted fires before any polling so the caller can map variant->job early.
{
  let pollsBeforeOnStarted = -1;
  const { invoke, calls } = recordingInvoke({
    poll_upload_states: [publishedSnapshot],
  });
  await startCampaignUploads(singleCampaign, [singleEntry], invoke, {
    sleep: noSleep,
    onStarted: (started: CampaignUploadRequest[]) => {
      assert.equal(started.length, 1);
      assert.equal(started[0].jobId, "job-youtube");
      pollsBeforeOnStarted = calls.filter(
        (c) => c.command === "poll_upload_states",
      ).length;
    },
  });
  assert.equal(pollsBeforeOnStarted, 0, "onStarted fires before polling");
}

console.log("campaign-publisher: OK");
