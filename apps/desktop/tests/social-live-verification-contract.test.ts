import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const readRepoFile = (path: string) => readFileSync(resolve(repoRoot, path), "utf8");

const contract = readRepoFile("docs/social-server/live-verification.html");
const readme = readRepoFile("docs/social-server/README.md");

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
  "social_update_target",
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
