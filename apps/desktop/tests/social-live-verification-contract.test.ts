import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";

const contract = readFileSync("docs/social-server/live-verification.html", "utf8");
const readme = readFileSync("docs/social-server/README.md", "utf8");

for (const provider of ["YouTube", "TikTok", "Instagram", "Twitter/X"]) {
  assert.match(
    contract,
    new RegExp(`<h3[^>]*>${provider.replace("/", "\\/")}</h3>`),
    `live verification contract must include ${provider}`,
  );
}

for (const phrase of [
  "OAuth sign-in",
  "private or sandbox publish",
  "scheduled app-closed firing",
  "provider URL",
  "audit history",
  "cleanup",
]) {
  assert.match(
    contract,
    new RegExp(phrase, "i"),
    `live verification contract must cover ${phrase}`,
  );
}

for (const command of [
  "social_accounts",
  "social_bind_target",
  "social_validate_target",
  "social_schedule_target",
  "social_upload_artifact",
  "social_publish_job",
]) {
  assert.match(
    contract,
    new RegExp(command),
    `live verification contract must reference ${command}`,
  );
}

assert.match(
  readme,
  /live-verification\.html/,
  "social server runbook must link the live verification contract",
);

console.log("social-live-verification-contract: OK");
