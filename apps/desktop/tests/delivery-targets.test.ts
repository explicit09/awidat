import assert from "node:assert/strict";
import {
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

console.log("delivery-targets: OK");
