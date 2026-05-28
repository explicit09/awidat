#!/usr/bin/env node
/**
 * Asserts that the renamed brand + pill family tokens are present in
 * tokens.css after the redesign migration. Fails loudly if a token is
 * dropped without an explicit replacement.
 */
import { readFileSync } from "node:fs";
import { strict as assert } from "node:assert";

const css = readFileSync(new URL("../src/ui/tokens.css", import.meta.url), "utf8");

const requiredTokens = [
  // brand
  "--color-brand: #FF7A18",
  "--color-brand-hover: #FF8B33",
  "--color-brand-active: #E5641A",
  "--color-accent-mint: #20C997",
  // job family
  "--color-job-idle-dot",
  "--color-job-running-dot",
  "--color-job-ready-dot",
  "--color-job-failed-dot",
  "--color-job-idle-fill",
  "--color-job-running-fill",
  "--color-job-ready-fill",
  "--color-job-failed-fill",
  "--color-job-idle-text",
  "--color-job-running-text",
  "--color-job-ready-text",
  "--color-job-failed-text",
  // proposal family
  "--color-proposal-proposed-dot",
  "--color-proposal-accepted-dot",
  "--color-proposal-rejected-dot",
  "--color-proposal-revised-dot",
];
const removedTokens = [
  // the dropped 11-family triplets — must NOT appear
  "--color-pill-proposed-fill",
  "--color-pill-pending-fill",
  "--color-pill-reviewing-fill",
  "--color-pill-missing-fill",
];

for (const t of requiredTokens) {
  assert.ok(css.includes(t), `tokens.css missing required token: ${t}`);
}
for (const t of removedTokens) {
  assert.ok(!css.includes(t), `tokens.css still contains removed token: ${t}`);
}

console.log(`tokens-presence: OK (${requiredTokens.length} required, ${removedTokens.length} removed verified)`);
