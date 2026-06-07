/**
 * Pure-logic tests for the server-backed social publishing model helpers.
 * No React, no Tauri — just the derivations the surfaces depend on.
 */
import { strict as assert } from "node:assert";

import {
  accountStatusLabel,
  eligibilitySummary,
  reasonCopy,
  jobStatusLabel,
  canCancel,
  canRetry,
  canReschedule,
  canReconnect,
  canViewAccountAudit,
  buildPlatformFieldsForPublish,
  isTerminal,
} from "../src/app/social/socialModel.ts";

// Account status labels.
assert.equal(accountStatusLabel("connected"), "Connected");
assert.equal(accountStatusLabel("needs_reauth"), "Needs reconnect");
assert.equal(accountStatusLabel("missing_scope"), "Missing permission");

// Eligibility summary.
assert.equal(eligibilitySummary({ eligible: true, reasons: [] }), "Eligible");
assert.equal(
  eligibilitySummary({ eligible: false, reasons: ["account_not_eligible"] }),
  "Not eligible — account not eligible",
);
assert.equal(eligibilitySummary({ eligible: false, reasons: [] }), "Not eligible");

// Reason copy falls back to a humanized code.
assert.equal(reasonCopy("scheduled_time_invalid"), "scheduled time is in the past");
assert.equal(
  reasonCopy("twitter_x_oauth_client_pending"),
  "twitter x oauth client pending",
);
assert.equal(reasonCopy("title.required"), "title required");
assert.equal(reasonCopy("some_unknown_code"), "some unknown code");

// Job status labels.
assert.equal(jobStatusLabel("processing"), "Processing");
assert.equal(jobStatusLabel("requires_action"), "Action needed");

// Cancel / retry gating.
assert.equal(canCancel("scheduled"), true);
assert.equal(canCancel("published"), false);
assert.equal(canCancel("cancelled"), false);
assert.equal(canRetry("failed"), true);
assert.equal(canRetry("requires_action"), true);
assert.equal(canRetry("scheduled"), false);
assert.equal(canReschedule("scheduled"), true);
assert.equal(canReschedule("uploading"), false);
assert.equal(canReschedule("published"), false);
assert.equal(canReconnect("needs_reauth"), true);
assert.equal(canReconnect("missing_scope"), true);
assert.equal(canReconnect("revoked"), true);
assert.equal(canReconnect("connected"), false);
assert.equal(canReconnect("disabled"), false);
assert.equal(canViewAccountAudit("connected"), true);
assert.equal(canViewAccountAudit("needs_reauth"), true);
assert.equal(canViewAccountAudit("disabled"), true);

// Manual publish field builder keeps full platform metadata.
assert.deepEqual(
  buildPlatformFieldsForPublish({
    provider: "youtube",
    privacy: "unlisted",
    title: "Episode title",
    description: "Episode description",
    tagsInput: "podcast, launch, podcast",
    thumbnailPath: "/tmp/thumb.jpg",
  }),
  {
    privacy: "unlisted",
    title: "Episode title",
    description: "Episode description",
    tags: ["podcast", "launch"],
    thumbnailRef: "file:///tmp/thumb.jpg",
  },
);
assert.deepEqual(
  buildPlatformFieldsForPublish({
    provider: "instagram",
    privacy: "private",
    title: "Ignored title",
    description: "Caption only",
    tagsInput: "",
    thumbnailPath: "",
  }),
  {
    privacy: "private",
    description: "Caption only",
    tags: [],
  },
);
assert.deepEqual(
  buildPlatformFieldsForPublish({
    provider: "instagram",
    privacy: "private",
    title: "Shared scheduler title",
    description: "",
    tagsInput: "",
    thumbnailPath: "",
  }),
  {
    privacy: "private",
    description: "Shared scheduler title",
    tags: [],
  },
);
assert.deepEqual(
  buildPlatformFieldsForPublish({
    provider: "twitter_x",
    privacy: "private",
    title: "Post text",
    description: "Ignored long description",
    tagsInput: "ignored",
    thumbnailPath: "/tmp/ignored-thumb.jpg",
  }),
  {
    title: "Post text",
  },
);

// Terminal-state detection (drives passive polling — firing is server-side now).
assert.equal(isTerminal("published"), true);
assert.equal(isTerminal("failed"), true);
assert.equal(isTerminal("cancelled"), true);
assert.equal(isTerminal("scheduled"), false);
assert.equal(isTerminal("uploading"), false);
assert.equal(isTerminal("processing"), false);
assert.equal(isTerminal("requires_action"), false);

console.log("social-model.test.ts: ok");
