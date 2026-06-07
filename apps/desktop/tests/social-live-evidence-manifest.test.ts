import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";

type ProviderEvidence = {
  provider: string;
  status: "pending" | "verified" | "blocked";
  evidenceFolder: string;
  evidenceIndex: string;
  checks: Record<string, boolean>;
  blockers: string[];
};

type EvidenceManifest = {
  version: number;
  allProvidersVerified: boolean;
  updatedAt: string;
  providers: ProviderEvidence[];
};

const manifest = JSON.parse(
  readFileSync("docs/social-server/live-evidence-manifest.json", "utf8"),
) as EvidenceManifest;
const readme = readFileSync("docs/social-server/README.md", "utf8");
const contract = readFileSync("docs/social-server/live-verification.html", "utf8");

assert.equal(manifest.version, 1);
assert.match(manifest.updatedAt, /^\d{4}-\d{2}-\d{2}$/);
assert.ok(
  manifest.updatedAt >= "2026-06-07",
  "manifest updatedAt must cover the metadataEdit evidence schema update",
);
assert.equal(
  manifest.allProvidersVerified,
  manifest.providers.every((entry) => entry.status === "verified"),
  "allProvidersVerified must reflect provider statuses",
);

const requiredProviders = ["youtube", "tiktok", "instagram", "twitter_x"];
assert.deepEqual(
  manifest.providers.map((entry) => entry.provider).sort(),
  [...requiredProviders].sort(),
);

const requiredChecks = [
  "oauthSignIn",
  "selectedAccount",
  "metadataValidation",
  "metadataEdit",
  "privateOrSandboxPublish",
  "scheduledAppClosedFiring",
  "statusPolling",
  "providerUrl",
  "auditHistory",
  "negativePath",
  "cleanup",
];

for (const entry of manifest.providers) {
  assert.match(entry.evidenceFolder, /^docs\/social-server\/evidence\//);
  assert.match(
    entry.evidenceIndex,
    new RegExp(`^${entry.evidenceFolder.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}README\\.md$`),
    `${entry.provider} evidence must point at a concrete README index`,
  );
  const evidenceIndex = readFileSync(entry.evidenceIndex, "utf8");
  assert.match(evidenceIndex, new RegExp(entry.provider));
  assert.match(evidenceIndex, /OAuth sign-in/i);
  assert.match(evidenceIndex, /metadata edit/i);
  assert.match(evidenceIndex, /scheduled app-closed firing/i);
  assert.match(evidenceIndex, /provider URL/i);
  for (const check of requiredChecks) {
    assert.equal(
      typeof entry.checks[check],
      "boolean",
      `${entry.provider} evidence must include ${check}`,
    );
  }
  if (entry.status !== "verified") {
    assert.ok(
      entry.blockers.length > 0,
      `${entry.provider} pending/blocked evidence must name blockers`,
    );
  } else {
    assert.deepEqual(
      Object.values(entry.checks),
      requiredChecks.map(() => true),
      `${entry.provider} verified evidence must have every check true`,
    );
    assert.deepEqual(
      entry.blockers,
      [],
      `${entry.provider} verified evidence must not retain blockers`,
    );
  }
}

assert.match(readme, /live-evidence-manifest\.json/);
assert.match(contract, /live-evidence-manifest\.json/);

console.log("social-live-evidence-manifest: OK");
