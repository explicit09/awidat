import { strict as assert } from "node:assert";

import {
  CAMPAIGN_MANIFEST_VERSION,
  approvalSummary,
  createCampaignManifest,
  createPlatformVariant,
  createPublishableItem,
  type CampaignManifest,
} from "../src/campaign/manifest.ts";

const now = 1_782_600_000_000;

const item = createPublishableItem({
  itemId: "item-short-1",
  kind: "short",
  title: "Founder Explains The AI Workflow",
  caption: "Founder explains the AI workflow #ai #startup",
  hashtags: ["ai", "startup"],
  artifactPath: "/tmp/short.mp4",
  durationS: 42,
  aspectRatio: "9:16",
  transcriptAnchors: [{ startS: 12.1, endS: 54.2, text: "This is the workflow." }],
  preflightChecks: [{ id: "duration.short", severity: "pass", message: "Short duration is valid." }],
});

const variant = createPlatformVariant({
  variantId: "variant-tiktok-1",
  itemId: "item-short-1",
  platform: "tiktok",
  accountId: "local-tiktok",
  scheduledFor: now + 86_400_000,
  platformFields: {
    privacy: "private",
    coverTimestampMs: 1000,
  },
});

const manifest: CampaignManifest = createCampaignManifest({
  campaignId: "campaign-1",
  sourceAssetId: "asset-1",
  campaignType: "shorts",
  title: "AI Workflow Shorts Campaign",
  items: [item],
  platformVariants: [variant],
  createdAt: now,
});

assert.equal(CAMPAIGN_MANIFEST_VERSION, 1);
assert.equal(manifest.version, 1);
assert.equal(manifest.approvalState, "draft");
assert.equal(manifest.items[0].approvalState, "draft");
assert.equal(manifest.platformVariants[0].status, "draft");
assert.equal(manifest.updatedAt, now);

const summary = approvalSummary(manifest);
assert.deepEqual(summary, {
  totalItems: 1,
  approvedItems: 0,
  totalVariants: 1,
  approvedVariants: 0,
  scheduledVariants: 1,
});

console.log("campaign-manifest: OK");
