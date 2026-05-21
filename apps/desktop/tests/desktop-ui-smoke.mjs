#!/usr/bin/env node
/**
 * Phase 2 smoke test — drives the new shell in a headless browser and
 * asserts the key elements render at each stage.
 *
 * Doesn't run inside Tauri (no native backend) — exercises the UI shell,
 * not the agent loop. Tauri-runtime side effects are guarded by `isTauri()`
 * checks in src/state/appGlue.ts.
 *
 * Requires `pnpm dev` running on :1420.
 */

import { chromium } from "playwright";
import { strict as assert } from "node:assert";

const URL = process.env.SMOKE_URL ?? "http://localhost:1420/";
const SCREENSHOT_DIR = process.env.SMOKE_OUT_DIR ?? "tests/smoke";

const browser = await chromium.launch();
const ctx = await browser.newContext({
  viewport: { width: 1400, height: 1000 },
  deviceScaleFactor: 2,
});

function makePage() {
  return ctx.newPage().then((page) => {
    const errors = [];
    page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
    page.on("console", (m) => {
      if (m.type() === "error") errors.push(`console.error: ${m.text()}`);
    });
    return { page, errors };
  });
}

async function setStage(page, stage) {
  await page.locator('button[role="tab"]', { hasText: stage }).first().click();
  await page.waitForTimeout(120);
}

const passes = [];
const failures = [];

async function check(name, fn) {
  try {
    await fn();
    passes.push(name);
    console.log(`  ok  ${name}`);
  } catch (err) {
    failures.push({ name, err });
    console.error(`  FAIL  ${name}\n    ${err.message}`);
  }
}

await check("shell loads with no console errors", async () => {
  const { page, errors } = await makePage();
  await page.goto(URL, { waitUntil: "networkidle" });
  await page.waitForTimeout(300);
  if (errors.length > 0) {
    throw new Error(`console errors: ${errors.join("\n")}`);
  }
  await page.screenshot({ path: `${SCREENSHOT_DIR}/01-loaded.png`, fullPage: false });
  await page.close();
});

await check("stage indicator shows all 6 stages", async () => {
  const { page } = await makePage();
  await page.goto(URL, { waitUntil: "networkidle" });
  const stageNames = await page.locator('button[role="tab"]').allTextContents();
  for (const expected of ["Intent", "Indexing", "Proposal", "Review", "Revise", "Deliver"]) {
    const found = stageNames.some((s) => s.toLowerCase().includes(expected.toLowerCase()));
    assert.ok(found, `stage "${expected}" missing — saw: ${stageNames.join(", ")}`);
  }
  await page.close();
});

await check("intent stage shows StageStub with primary CTA", async () => {
  const { page } = await makePage();
  await page.goto(URL, { waitUntil: "networkidle" });
  await setStage(page, "Intent");
  const stub = await page.textContent("body");
  assert.ok(stub.includes("Intent"), "Intent title missing");
  assert.ok(stub.includes("Open Proposal stage"), "primary CTA missing");
  await page.screenshot({ path: `${SCREENSHOT_DIR}/02-intent.png`, fullPage: false });
  await page.close();
});

await check("proposal stage renders all 4 surface regions", async () => {
  const { page } = await makePage();
  await page.goto(URL, { waitUntil: "networkidle" });
  await setStage(page, "Proposal");
  const body = await page.textContent("body");
  assert.ok(body.toUpperCase().includes("COMMAND"), "Command Rail missing");
  // PreviewSurface header includes a "PROPOSAL" uppercase label;
  // body text concatenation can drop it (uppercase via CSS not text content).
  // Look for the uppercase chip label by attribute instead.
  const propLabel = await page
    .locator(":text-matches('PROPOSAL', 'i')")
    .count();
  assert.ok(propLabel > 0, "Preview header label missing");
  assert.ok(body.includes("Timeline"), "Timeline tab missing");
  assert.ok(body.includes("Transcript"), "Transcript tab missing");
  assert.ok(body.toUpperCase().includes("PROPOSAL INSPECTOR"), "Inspector header missing");
  await page.screenshot({ path: `${SCREENSHOT_DIR}/03-proposal.png`, fullPage: false });
  await page.close();
});

await check("proposal stage's lens nav exposes all 9 lenses", async () => {
  const { page } = await makePage();
  await page.goto(URL, { waitUntil: "networkidle" });
  await setStage(page, "Proposal");
  const lensNames = await page.locator('nav button[role="tab"]').allTextContents();
  for (const expected of [
    "Import",
    "Index",
    "Selects",
    "Assembly",
    "Review",
    "Captions",
    "Audio",
    "Color",
    "Delivery",
  ]) {
    const found = lensNames.some((s) => s.includes(expected));
    assert.ok(found, `lens "${expected}" missing`);
  }
  await page.close();
});

await check("Intent stage shows StageStub (Phase 2.10)", async () => {
  const { page } = await makePage();
  await page.goto(URL, { waitUntil: "networkidle" });
  await setStage(page, "Intent");
  const body = await page.textContent("body");
  assert.ok(
    body.includes("Open Proposal stage"),
    "Intent stub missing primary CTA",
  );
  await page.close();
});

await check("Review stage shows the transcript surface (Phase 3.2)", async () => {
  const { page } = await makePage();
  await page.goto(URL, { waitUntil: "networkidle" });
  await setStage(page, "Review");
  const body = await page.textContent("body");
  assert.ok(body.includes("No transcript yet"), "Review surface empty state missing");
  await page.screenshot({ path: `${SCREENSHOT_DIR}/04-review.png`, fullPage: false });
  await page.close();
});

await check("Revise stage shows the revision composer (Phase 3.4)", async () => {
  const { page } = await makePage();
  await page.goto(URL, { waitUntil: "networkidle" });
  await setStage(page, "Revise");
  const body = await page.textContent("body");
  assert.ok(body.includes("Revise the working timeline"), "Revise surface title missing");
  assert.ok(body.includes("Ask agent"), "Revise CTA missing");
  await page.screenshot({ path: `${SCREENSHOT_DIR}/05-revise.png`, fullPage: false });
  await page.close();
});

await check("Indexing stage shows the dashboard (Phase 4.1)", async () => {
  const { page } = await makePage();
  await page.goto(URL, { waitUntil: "networkidle" });
  await setStage(page, "Indexing");
  const body = await page.textContent("body");
  assert.ok(body.includes("Indexing pipeline"), "Indexing pipeline header missing");
  assert.ok(body.includes("Ask agent for first cut"), "Hand-off CTA missing");
  await page.screenshot({ path: `${SCREENSHOT_DIR}/06-indexing.png`, fullPage: false });
  await page.close();
});

await check("Indexing dashboard names all 9 indexing tasks", async () => {
  const { page } = await makePage();
  await page.goto(URL, { waitUntil: "networkidle" });
  await setStage(page, "Indexing");
  const body = await page.textContent("body");
  for (const task of [
    "Transcripts",
    "Scenes",
    "Audio analysis",
    "Face detection",
    "Motion analysis",
    "Color analysis",
    "Silence detection",
    "Speaker diarization",
    "Caption readiness",
  ]) {
    assert.ok(body.includes(task), `indexing task "${task}" missing`);
  }
  await page.close();
});

await check("Deliver stage shows the delivery surface (Phase 4.3)", async () => {
  const { page } = await makePage();
  await page.goto(URL, { waitUntil: "networkidle" });
  await setStage(page, "Deliver");
  const body = await page.textContent("body");
  assert.ok(body.includes("Targets"), "Delivery targets pane missing");
  assert.ok(body.includes("Preflight"), "Preflight pane missing");
  assert.ok(body.includes("Render summary"), "Render summary missing");
  // 6 target presets all named
  for (const t of ["YouTube", "TikTok", "Instagram", "Captions", "Cover", "Custom frame"]) {
    assert.ok(body.includes(t), `delivery target "${t}" missing`);
  }
  await page.screenshot({ path: `${SCREENSHOT_DIR}/07-deliver.png`, fullPage: false });
  await page.close();
});

await check("status footer shows agent + local + version", async () => {
  const { page } = await makePage();
  await page.goto(URL, { waitUntil: "networkidle" });
  const body = await page.textContent("body");
  assert.ok(body.includes("Agent"), "agent footer missing");
  assert.ok(body.includes("local"), "local footer missing");
  assert.ok(body.includes("ui-v2"), "version footer missing");
  await page.close();
});

await browser.close();

console.log(`\n${passes.length} passed, ${failures.length} failed`);
if (failures.length > 0) {
  process.exit(1);
}
