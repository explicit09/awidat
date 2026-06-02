import { strict as assert } from "node:assert";

import {
  createCampaignManifest,
  createPlatformVariant,
  createPublishableItem,
} from "../src/campaign/manifest.ts";
import { campaignStoreSelectors, useCampaignStore } from "../src/campaign/store.ts";

function resetStore(): void {
  useCampaignStore.setState({ campaigns: [] });
}

const campaign = createCampaignManifest({
  campaignId: "campaign-store-1",
  sourceAssetId: "asset-1",
  campaignType: "podcast",
  title: "Podcast Campaign",
  createdAt: 1_782_600_000_000,
  items: [
    createPublishableItem({
      itemId: "item-long",
      kind: "long_form",
      title: "Full Episode",
      artifactPath: "/tmp/episode.mp4",
    }),
  ],
  platformVariants: [
    createPlatformVariant({
      variantId: "variant-youtube",
      itemId: "item-long",
      platform: "youtube",
    }),
  ],
});

resetStore();
useCampaignStore.getState().upsertCampaign(campaign);
assert.equal(useCampaignStore.getState().campaigns.length, 1);
assert.equal(campaignStoreSelectors.active(useCampaignStore.getState())?.campaignId, "campaign-store-1");

useCampaignStore.getState().approveItem("campaign-store-1", "item-long");
let saved = useCampaignStore.getState().campaigns[0];
assert.equal(saved.items[0].approvalState, "approved");

useCampaignStore.getState().approveVariant("campaign-store-1", "variant-youtube");
saved = useCampaignStore.getState().campaigns[0];
assert.equal(saved.platformVariants[0].status, "approved");
assert.equal(saved.approvalState, "approved");

useCampaignStore
  .getState()
  .setVariantPublishJob("campaign-store-1", "variant-youtube", "render-job-1");
saved = useCampaignStore.getState().campaigns[0];
assert.equal(saved.platformVariants[0].status, "uploading");
assert.equal(saved.platformVariants[0].publishJobId, "render-job-1");

useCampaignStore.getState().requestChanges("campaign-store-1", "item-long");
saved = useCampaignStore.getState().campaigns[0];
assert.equal(saved.items[0].approvalState, "changes_requested");
assert.equal(saved.approvalState, "changes_requested");

useCampaignStore.getState().removeCampaign("campaign-store-1");
assert.equal(useCampaignStore.getState().campaigns.length, 0);

console.log("campaign-store: OK");
