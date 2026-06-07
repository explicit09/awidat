import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";

const flyToml = readFileSync("crates/social-server/fly.toml", "utf8");
const readme = readFileSync("docs/social-server/README.md", "utf8");

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

console.log("social-server-deploy-config: OK");
