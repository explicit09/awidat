import { strict as assert } from "node:assert";

import type { RenderQueueEntry } from "../src/app/renderQueue.ts";
import {
  createCampaignManifest,
  createPlatformVariant,
  createPublishableItem,
} from "../src/campaign/manifest.ts";
import { campaignUploadRequests } from "../src/campaign/publisher.ts";

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

console.log("campaign-publisher: OK");
