// Tests for the AI-disclosure inspection store and display helpers.
//
// We don't mount React — the helpers we care about (`truncatePrompt`,
// `summarizeCredit`, `emptyAiDisclosure`) are pure, and the store
// actions are exercised directly through the same pattern
// `render-queue-upload.test.ts` uses.

import { strict as assert } from "node:assert";

import {
  emptyAiDisclosure,
  summarizeCredit,
  truncatePrompt,
  useAiDisclosure,
  type AiDisclosure,
  type GeneratedMediaCredit,
} from "../src/state/aiDisclosure.ts";

function resetStore(): void {
  useAiDisclosure.setState({
    byJobId: {},
  });
}

// ---- emptyAiDisclosure is clean ----
{
  const empty = emptyAiDisclosure();
  assert.equal(empty.has_synthetic_content, false);
  assert.deepEqual(empty.credits, []);
}

// ---- truncatePrompt caps at 80 chars by default ----
{
  const short = "neon city at dusk";
  assert.equal(truncatePrompt(short), short);
  const long = "x".repeat(120);
  const truncated = truncatePrompt(long);
  assert.ok(truncated.length <= 80, `truncated length: ${truncated.length}`);
  assert.ok(truncated.endsWith("…"));
}

// ---- truncatePrompt accepts a custom max ----
{
  const long = "x".repeat(50);
  const truncated = truncatePrompt(long, 20);
  assert.ok(truncated.length <= 20);
  assert.ok(truncated.endsWith("…"));
}

// ---- summarizeCredit folds provider/model + truncated prompt ----
{
  const credit: GeneratedMediaCredit = {
    provider: "runway",
    model: "gen3",
    prompt: "a quiet street at dusk with neon signs reflecting in puddles",
    asset_id: "job-1",
  };
  const summary = summarizeCredit(credit);
  assert.ok(summary.startsWith("runway/gen3 · "));
  assert.ok(summary.includes("quiet street"));
}

// ---- summarizeCredit omits model when missing ----
{
  const credit: GeneratedMediaCredit = {
    provider: "mock",
    prompt: "short",
    asset_id: "j",
  };
  assert.equal(summarizeCredit(credit), "mock · short");
}

// ---- summarizeCredit handles empty prompt ----
{
  const credit: GeneratedMediaCredit = {
    provider: "mock",
    prompt: "",
    asset_id: "j",
  };
  assert.equal(summarizeCredit(credit), "mock");
}

// ---- store set/get round-trips ----
{
  resetStore();
  const disclosure: AiDisclosure = {
    has_synthetic_content: true,
    credits: [
      {
        provider: "runway",
        model: "gen3",
        prompt: "neon city",
        generated_at: 1_700_000_000,
        asset_id: "job-1",
      },
    ],
  };
  useAiDisclosure.getState().set("render-001", disclosure);
  const stored = useAiDisclosure.getState().byJobId["render-001"];
  assert.deepEqual(stored, disclosure);
}

// ---- store forget removes one entry ----
{
  resetStore();
  const disclosure: AiDisclosure = {
    has_synthetic_content: true,
    credits: [
      { provider: "mock", prompt: "x", asset_id: "a" },
    ],
  };
  const store = useAiDisclosure.getState();
  store.set("render-A", disclosure);
  store.set("render-B", disclosure);
  store.forget("render-A");
  const state = useAiDisclosure.getState();
  assert.equal(state.byJobId["render-A"], undefined);
  assert.ok(state.byJobId["render-B"]);
}

// ---- has_synthetic_content false → banner-side check ----
{
  // The banner reads `disclosure.has_synthetic_content` to decide
  // whether to render — we sanity-check the shape is preserved.
  const clean = emptyAiDisclosure();
  assert.equal(clean.has_synthetic_content, false);
  assert.equal(clean.credits.length, 0);
}

console.log("ai-disclosure: OK");
