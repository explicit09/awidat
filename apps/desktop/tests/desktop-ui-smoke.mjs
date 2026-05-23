#!/usr/bin/env node
/**
 * Desktop UI smoke test.
 *
 * This runs the browser/static app path used for design review. With no Tauri
 * project open, the app should boot directly into the Screen 2 golden cockpit:
 * app chrome, workflow lenses, agent command rail, proposal preview, timeline,
 * and proposal inspector.
 *
 * Requires `pnpm dev` running on :1420.
 */

import { chromium } from "playwright";
import { strict as assert } from "node:assert";
import { mkdirSync } from "node:fs";

const BASE_URL = process.env.SMOKE_URL ?? "http://localhost:1420/";
const SCREENSHOT_DIR = process.env.SMOKE_OUT_DIR ?? "tests/smoke";
mkdirSync(SCREENSHOT_DIR, { recursive: true });

const browser = await chromium.launch();
const ctx = await browser.newContext({
  viewport: { width: 1586, height: 992 },
  deviceScaleFactor: 1,
});

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

async function makePage() {
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(`console.error: ${m.text()}`);
  });
  return { page, errors };
}

function demoUrl(screen) {
  const url = new URL(BASE_URL);
  url.searchParams.set("awidatScreen", screen);
  return url.toString();
}

function conceptUrl() {
  return new URL("/design/concept", BASE_URL).toString();
}

const SPEC_SCREENS = [
  {
    id: "screen1",
    url: conceptUrl(),
    text: [
      "Product Concept & Information Architecture",
      "AI-assisted podcast editing",
      "Evidence & Feedback Loop",
      "Information architecture",
      "Primary workspace model",
      "Stage-driven workflow",
    ],
  },
  {
    id: "screen2",
    url: BASE_URL,
    text: [
      "Main Desktop Workspace",
      "Agent Command",
      "Podcast Tightening v1",
      "Proposal Inspector",
      "Timeline",
      "Vedit",
      "Evidence",
    ],
  },
  {
    id: "screen4",
    text: ["Timeline / Transcript Hybrid", "Selected sentence", "Review · Transcript", "What this cut does", "Keep this pause"],
  },
  {
    id: "screen5",
    text: ["Cut / Proposal Inspector", "CUT 12 · L-cut", "Current timeline", "Proposed timeline", "Render output context", "Inspect deeper"],
  },
  {
    id: "screen6",
    text: ["Import / Indexing State", "Add media", "Add files", "Indexing pipeline", "Speaker diarization", "Ask agent for first cut"],
  },
  {
    id: "screen7",
    text: ["Delivery / Preflight State", "Targets", "Preflight", "Render summary", "Delivery confidence", "Export now"],
  },
  {
    id: "screen8",
    text: ["Empty / Loading / Error States", "No media imported", "Indexing media", "Proposal generation is blocked", "Repair with agent"],
  },
  {
    id: "screen9",
    text: ["Component System", "Proposal cards", "Timeline change markers", "Agent status", "Render / preflight findings", "Semantic color palette"],
  },
];

function screenUrl(screen) {
  return screen.url ?? demoUrl(screen.id);
}

await check("golden cockpit loads without console errors", async () => {
  const { page, errors } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  await page.waitForTimeout(300);
  assert.deepEqual(errors, []);
  await page.screenshot({ path: `${SCREENSHOT_DIR}/screen2-golden.png`, fullPage: false });
  await page.close();
});

for (const screen of SPEC_SCREENS) {
  await check(`${screen.id} loads and screenshots`, async () => {
    const { page, errors } = await makePage();
    await page.goto(screenUrl(screen), { waitUntil: "networkidle" });
    await page.waitForTimeout(300);
    assert.deepEqual(errors, []);
    const body = (await page.textContent("body")).toLowerCase();
    for (const expected of screen.text) {
      assert.ok(body.includes(expected.toLowerCase()), `${screen.id} missing text: ${expected}`);
    }
    if (screen.id === "screen1") {
      assert.equal(await page.locator("body > header").count(), 0, "screen1 should not render the app shell header");
      assert.ok(page.url().includes("/design/concept"), "screen1 should be reachable at /design/concept");
    }
    await page.screenshot({ path: `${SCREENSHOT_DIR}/${screen.id}.png`, fullPage: false });
    await page.close();
  });
}

await check("spec demo screens fit a compact desktop window", async () => {
  const compact = await browser.newContext({
    viewport: { width: 1280, height: 800 },
    deviceScaleFactor: 1,
  });
  try {
    for (const screen of SPEC_SCREENS) {
      const page = await compact.newPage();
      const errors = [];
      page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
      page.on("console", (m) => {
        if (m.type() === "error") errors.push(`console.error: ${m.text()}`);
      });
      await page.goto(screenUrl(screen), { waitUntil: "networkidle" });
      await page.waitForTimeout(150);
      assert.deepEqual(errors, []);
      const metrics = await page.evaluate(() => ({
        scrollW: document.documentElement.scrollWidth,
        clientW: document.documentElement.clientWidth,
        scrollH: document.documentElement.scrollHeight,
        clientH: document.documentElement.clientHeight,
      }));
      assert.equal(metrics.scrollW, metrics.clientW, `${screen.id} horizontally overflows compact viewport`);
      assert.equal(metrics.scrollH, metrics.clientH, `${screen.id} vertically overflows compact viewport`);
      if (screen.id === "screen2") {
        await page.screenshot({ path: `${SCREENSHOT_DIR}/screen2-compact.png`, fullPage: false });
      }
      await page.close();
    }
  } finally {
    await compact.close();
  }
});

await check("top chrome matches Screen 2 app model", async () => {
  const { page } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  const body = await page.textContent("body");
  for (const expected of [
    "Awidat",
    "Main Desktop Workspace",
    "Edit",
    "Deliver",
    "Awaiting review",
  ]) {
    assert.ok(body.includes(expected), `missing top chrome text: ${expected}`);
  }
  await page.getByTitle("Settings").waitFor({ state: "visible" });
  const clippedTopNav = await page.evaluate(() => {
    const header = document.querySelector("header");
    if (!header) return ["missing header"];
    const labels = ["Edit", "Deliver"];
    const clipped = [];
    const buttons = Array.from(header.querySelectorAll('button[role="tab"]'));
    for (const label of labels) {
      const button = buttons.find((el) => el.textContent?.trim() === label);
      if (!button) {
        clipped.push(`${label}: missing`);
        continue;
      }
      const rect = button.getBoundingClientRect();
      let node = button.parentElement;
      while (node && node !== header.parentElement) {
        const style = getComputedStyle(node);
        if (style.overflow !== "visible" || style.overflowX !== "visible") {
          const clip = node.getBoundingClientRect();
          if (rect.left < clip.left - 0.5 || rect.right > clip.right + 0.5) {
            clipped.push(`${label}: clipped by ${node.tagName.toLowerCase()}`);
            break;
          }
        }
        node = node.parentElement;
      }
    }
    return clipped;
  });
  assert.deepEqual(clippedTopNav, []);
  await page.close();
});

await check("top stage row is the only global workflow navigation", async () => {
  const { page } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  const lensRows = await page.locator('[role="tablist"][aria-label="Workflow lens"]').count();
  assert.equal(lensRows, 0, "secondary workflow lens row should not render");
  const stageNames = await page.locator('[role="tablist"][aria-label="Stage"] button[role="tab"]').allTextContents();
  for (const expected of [
    "Edit",
    "Deliver",
  ]) {
    assert.ok(stageNames.some((s) => s.includes(expected)), `stage "${expected}" missing`);
  }
  for (const folded of ["Intent", "Index", "Proposal", "Review", "Revise", "Import", "Selects", "Assembly", "Captions", "Audio", "Color"]) {
    assert.ok(!stageNames.some((s) => s.includes(folded)), `${folded} should not be a second-level global tab`);
  }
  await page.close();
});

await check("indexing dashboard treats import as a secondary add-media action", async () => {
  const { page } = await makePage();
  await page.goto(demoUrl("screen6"), { waitUntil: "networkidle" });
  const body = await page.textContent("body");
  for (const expected of [
    "Project media",
    "Add media",
    "Add files",
    "Add URL",
    "9 items",
  ]) {
    assert.ok(body.includes(expected), `missing indexing dashboard text: ${expected}`);
  }
  for (const stale of [
    "Project & import",
    "Importing 9 of 9 files",
    "12.4 GB / 12.4 GB",
  ]) {
    assert.ok(!body.includes(stale), `stale import dashboard text should be gone: ${stale}`);
  }
  await page.close();
});

await check("indexing dashboard keeps secondary insights compact and data-backed", async () => {
  const { page } = await makePage();
  await page.goto(demoUrl("screen6"), { waitUntil: "networkidle" });
  const body = await page.textContent("body");
  for (const expected of [
    "Index insights",
    "Structure preview",
    "00:42:11",
    "31",
    "126",
    "2",
  ]) {
    assert.ok(body.includes(expected), `missing indexing insight text: ${expected}`);
  }
  assert.ok(!body.includes("Smart hints"), "old smart hints heading should not be visible");
  await page.close();
});

await check("agent command rail renders Screen 2 intent, context, plan, activity, suggestions", async () => {
  const { page } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  const body = (await page.textContent("body")).toLowerCase();
  for (const expected of [
    "agent command",
    "Cut this into a tight 8-minute podcast episode.",
    "Remove dead air but preserve natural pacing.",
    "Clip: Interview_A",
    "Range: 00:12-18:40",
    "Transcript region selected",
    "Target: YouTube 16:9",
    "Building proposal...",
    "Est. time remaining",
    "00:01:48",
    "Build assembly (rough cut)",
    "activity",
    "suggested next actions",
    "Inspect 12 changed regions",
  ]) {
    assert.ok(body.includes(expected.toLowerCase()), `missing command rail text: ${expected}`);
  }
  await page.close();
});

await check("proposal preview renders before/after review controls", async () => {
  const { page } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  const body = (await page.textContent("body")).toLowerCase();
  for (const expected of [
    "proposal",
    "Podcast Tightening v1",
    "12 pending changes",
    "Before / After",
    "Side by Side",
    "active proposal overlay",
    "jump to change",
    "07",
  ]) {
    assert.ok(body.includes(expected.toLowerCase()), `missing preview text: ${expected}`);
  }
  await page.close();
});

await check("timeline and inspector expose review evidence", async () => {
  const { page } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  const body = (await page.textContent("body")).toLowerCase();
  for (const expected of [
    "Timeline",
    "Transcript",
    "Changes",
    "Evidence",
    "Proposed Timeline",
    "Current Timeline",
    "proposal inspector",
    "Cut 07 · J-cut",
    "Transcript boundary",
    "Audio energy drop",
    "Speaker handoff",
    "Visual continuity",
    "Accept",
    "Reject",
    "Revise",
  ]) {
    assert.ok(body.includes(expected.toLowerCase()), `missing timeline/inspector text: ${expected}`);
  }
  await page.close();
});

await check("timeline dock avoids duplicate transcript and video lanes", async () => {
  const { page } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  const dock = page.locator(".edit-lower-dock");
  const dockText = (await dock.textContent()).toLowerCase();
  for (const expected of ["timeline", "changes", "evidence", "audio"]) {
    assert.ok(dockText.includes(expected), `missing simplified timeline text: ${expected}`);
  }
  for (const removed of ["selects", "channel lanes", "agent edits", "diff"]) {
    assert.ok(!dockText.includes(removed), `duplicate lower timeline text should be gone: ${removed}`);
  }
  const dockButtons = await dock.getByRole("button").allTextContents();
  assert.ok(dockButtons.some((label) => label.trim() === "Transcript"), "Transcript should remain a dock-level panel");
  assert.ok(!dockButtons.some((label) => label.trim() === "Selects"), "Selects should not remain as a lower timeline tab");
  await page.close();
});

await check("edit dock exposes transcript and Vedit without leaving Edit", async () => {
  const { page, errors } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  await page.locator(".edit-dock-header").getByRole("button", { name: "Transcript" }).click();
  let body = (await page.textContent("body")).toLowerCase();
  assert.ok(
    body.includes("no transcript") || body.includes("loading transcript") || body.includes("segments"),
    "transcript panel should be reachable",
  );
  await page.locator(".edit-dock-header").getByRole("button", { name: "Vedit" }).click();
  body = (await page.textContent("body")).toLowerCase();
  assert.ok(body.includes("timeline history") || body.includes("no vedit commits yet"), "Vedit panel should be reachable");
  await page.getByTitle("Collapse panel").click();
  body = (await page.textContent("body")).toLowerCase();
  assert.ok(body.includes("vedit panel collapsed"), "collapsed dock state should be visible");
  await page.getByRole("button", { name: "Pop out" }).click();
  const dialog = page.getByRole("dialog", { name: "Vedit popout" });
  await dialog.waitFor({ state: "visible" });
  const box = await dialog.boundingBox();
  assert.ok(box && box.y < 120 && box.height > 400, "Vedit popout should be visible in the viewport");
  assert.deepEqual(errors, []);
  await page.close();
});

await check("footer exposes model, autosave, render, and disk status", async () => {
  const { page } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  const body = (await page.textContent("body")).toLowerCase();
  for (const expected of [
    "agent online",
    "model: awidat pro 1.2",
    "context window: 42m",
    "autosaved 12:42:18",
    "render queue 1",
    "disk 1.2 tb free",
  ]) {
    assert.ok(body.includes(expected.toLowerCase()), `missing footer text: ${expected}`);
  }
  await page.close();
});

await browser.close();

console.log(`\n${passes.length} passed, ${failures.length} failed`);
if (failures.length > 0) {
  process.exit(1);
}
