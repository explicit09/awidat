import { strict as assert } from "node:assert";

import type { RenderQueueEntry } from "../src/app/renderQueue.ts";
import { planCampaignFromDelivery } from "../src/campaign/planner.ts";

const entries: RenderQueueEntry[] = [
  {
    id: "render-youtube",
    targetId: "youtube",
    label: "YouTube",
    kind: "video_master",
    status: "done",
    outputPath: "/tmp/youtube.mp4",
    reviewStatus: "approved",
    enqueuedAt: 1,
    uploadAccountIds: { youtube: "acct-youtube" },
  },
  {
    id: "render-tiktok",
    targetId: "tiktok",
    label: "TikTok",
    kind: "video_reframe",
    status: "done",
    outputPath: "/tmp/tiktok.mp4",
    reviewStatus: "pending",
    enqueuedAt: 2,
    uploadAccountIds: {
      tiktok: "acct-tiktok",
      instagram: "acct-instagram",
      twitter_x: "acct-twitter",
    },
    uploadMetadata: {
      tiktok: {
        title: "TikTok title",
        description: "TikTok caption",
        tags: ["ai", "workflow"],
        visibility: "public",
        scheduledAt: 1_782_686_400,
      },
    },
  },
];

const campaign = planCampaignFromDelivery({
  campaignType: "podcast",
  sourceAssetId: "asset-123",
  title: "Episode rollout",
  selectedTargets: ["youtube", "tiktok", "instagram", "twitter_x"],
  renderEntries: entries,
  createdAt: 1_782_600_000_000,
});

assert.equal(campaign.campaignType, "podcast");
assert.equal(campaign.items.length, 2);
assert.equal(campaign.platformVariants.length, 4);

const youtubeItem = campaign.items.find((item) => item.itemId === "item-render-youtube");
assert.ok(youtubeItem);
assert.equal(youtubeItem.kind, "long_form");
assert.equal(youtubeItem.artifactPath, "/tmp/youtube.mp4");
assert.equal(youtubeItem.approvalState, "approved");

const tiktokItem = campaign.items.find((item) => item.itemId === "item-render-tiktok");
assert.ok(tiktokItem);
assert.equal(tiktokItem.kind, "short");
assert.equal(tiktokItem.aspectRatio, "9:16");

const instagramVariant = campaign.platformVariants.find((variant) => variant.platform === "instagram");
assert.ok(instagramVariant);
assert.equal(instagramVariant.status, "draft");
assert.equal(instagramVariant.itemId, "item-render-tiktok");

const twitterXVariant = campaign.platformVariants.find((variant) => variant.platform === "twitter_x");
assert.ok(twitterXVariant);
assert.equal(twitterXVariant.status, "draft");
assert.equal(twitterXVariant.itemId, "item-render-tiktok");

const tiktokVariant = campaign.platformVariants.find((variant) => variant.platform === "tiktok");
assert.ok(tiktokVariant);
assert.equal(tiktokVariant.accountId, "acct-tiktok");
assert.equal(tiktokVariant.scheduledFor, 1_782_686_400);
assert.deepEqual(tiktokVariant.platformFields, {
  title: "TikTok title",
  description: "TikTok caption",
  tags: "ai,workflow",
  visibility: "public",
});

const youtubeVariant = campaign.platformVariants.find((variant) => variant.platform === "youtube");
assert.ok(youtubeVariant);
assert.equal(youtubeVariant.accountId, "acct-youtube");
assert.equal(instagramVariant.accountId, "acct-instagram");
assert.equal(twitterXVariant.accountId, "acct-twitter");

console.log("campaign-planner: OK");
