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

// Terminal-state detection (drives passive polling — firing is server-side now).
assert.equal(isTerminal("published"), true);
assert.equal(isTerminal("failed"), true);
assert.equal(isTerminal("cancelled"), true);
assert.equal(isTerminal("scheduled"), false);
assert.equal(isTerminal("uploading"), false);
assert.equal(isTerminal("processing"), false);
assert.equal(isTerminal("requires_action"), false);

console.log("social-model.test.ts: ok");
