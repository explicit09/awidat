import { strict as assert } from "node:assert";

import {
  PROVIDERS,
  VISIBLE_PROVIDERS,
  providerDisplayName,
} from "../src/app/publishingSettingsModel.ts";

assert.deepEqual(
  PROVIDERS.map((provider) => provider.key),
  ["youtube", "tiktok", "instagram", "twitter_x"],
);
assert.deepEqual(
  VISIBLE_PROVIDERS.map((provider) => provider.key),
  ["youtube", "twitter_x"],
  "settings hides providers whose production upload path is not ready",
);
assert.deepEqual(
  PROVIDERS.map((provider) => providerDisplayName(provider.key)),
  ["YouTube", "TikTok", "Instagram", "Twitter/X"],
);

console.log("publishing-settings: OK");
