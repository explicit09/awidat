import assert from "node:assert/strict";
import {
  VISIBLE_DELIVERY_TARGET_KEYS,
  normalizeVisibleDeliveryTargetKeys,
  renderQueueLabelForTarget,
  type DeliveryTargetKey,
} from "../src/app/deliveryTargets.ts";

const tiktokOnly = new Set<DeliveryTargetKey>(["tiktok"]);
assert.equal(
  renderQueueLabelForTarget("youtube", tiktokOnly),
  "Source master render",
  "implicit master render should not be labeled as a YouTube destination",
);

const youtubeSelected = new Set<DeliveryTargetKey>(["youtube", "tiktok"]);
assert.equal(
  renderQueueLabelForTarget("youtube", youtubeSelected),
  "YouTube",
  "selected YouTube target should keep the YouTube queue label",
);

assert.deepEqual(
  VISIBLE_DELIVERY_TARGET_KEYS,
  ["youtube", "twitter_x", "captions", "cover", "custom"],
  "MVP delivery UI should hide TikTok and Instagram until those paths are real",
);

assert.deepEqual(
  normalizeVisibleDeliveryTargetKeys([
    "youtube",
    "tiktok",
    "instagram",
    "twitter_x",
  ]),
  ["youtube", "twitter_x"],
  "persisted hidden delivery targets should be cleared on load",
);

console.log("delivery-targets: OK");
