import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const readRepoFile = (path: string) => readFileSync(resolve(repoRoot, path), "utf8");

const flyToml = readRepoFile("crates/social-server/fly.toml");
const runLocal = readRepoFile("crates/social-server/run-local.sh");
const readme = readRepoFile("docs/social-server/README.md");

assert.match(
  flyToml,
  /SOCIAL_FIRING_ENABLED\s*=\s*"true"/,
  "Fly deployment must enable server-side firing so scheduled posts run while the desktop app is closed.",
);
assert.doesNotMatch(
  readme,
  /SOCIAL_FIRING_ENABLED=false — skipping/,
  "Deployment smoke docs must not accept the disabled no-op worker as the ready state.",
);
assert.match(
  readme,
  /montage-publish-tick/,
  "Deployment docs must include the pg_cron publish tick schedule.",
);

for (const envName of [
  "GOOGLE_CLIENT_ID",
  "GOOGLE_CLIENT_SECRET",
  "TIKTOK_CLIENT_KEY",
  "TIKTOK_CLIENT_SECRET",
  "INSTAGRAM_CLIENT_ID",
  "INSTAGRAM_CLIENT_SECRET",
  "TWITTER_X_CLIENT_ID",
  "TWITTER_X_CLIENT_SECRET",
  "SOCIAL_ALLOWED_USER_IDS",
]) {
  assert.match(
    readme,
    new RegExp(`\\| \`${envName}\` \\|`),
    `Deployment docs must document ${envName}.`,
  );
  assert.match(
    runLocal,
    new RegExp(`export ${envName}=`),
    `Local social-server runner must pass through ${envName}.`,
  );
}

const envRows = Array.from(readme.matchAll(/^\| `([A-Z0-9_]+)` \|/gm)).map(
  (match) => match[1],
);
assert.deepEqual(
  envRows,
  [...new Set(envRows)],
  "Deployment docs must not duplicate environment variable rows.",
);

console.log("social-server-deploy-config: OK");
