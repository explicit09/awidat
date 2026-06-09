import { strict as assert } from "node:assert";

import {
  providerKeyActionLabel,
  providerKeyStatusLabel,
  providerKeySubtitle,
  type ProviderKeyRow,
} from "../src/app/providerKeysSettingsModel.ts";

const configured: ProviderKeyRow = {
  key: "openrouter",
  label: "OpenRouter",
  account: "openrouter_api_key",
  envVar: "OPENROUTER_API_KEY",
  capability: "Generated media",
  status: "configured",
  redacted: "sk-...7890",
};

assert.equal(providerKeyStatusLabel(configured), "Configured");
assert.equal(providerKeyActionLabel(configured), "Replace");
assert.equal(
  providerKeySubtitle(configured),
  "Generated media · OPENROUTER_API_KEY",
);

const missing: ProviderKeyRow = {
  ...configured,
  status: "notSet",
  redacted: null,
};

assert.equal(providerKeyStatusLabel(missing), "Not set");
assert.equal(providerKeyActionLabel(missing), "Add");

console.log("provider-keys-settings-model: OK");
