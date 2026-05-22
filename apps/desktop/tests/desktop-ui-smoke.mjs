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
      "Workflow lenses",
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
      "Evidence",
    ],
  },
  {
    id: "screen3",
    text: ["Agent Proposal Review", "Agent command history", "Proposed changes", "Batch insights", "Revise with prompt"],
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
    text: ["Import / Indexing State", "Import files", "Indexing pipeline", "Speaker diarization", "Ask agent for first cut"],
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
    "Project",
    "Workspace",
    "Agent",
    "Media",
    "Review",
    "Deliver",
    "Settings",
    "Interview_A",
    "Podcast Episode",
  ]) {
    assert.ok(body.includes(expected), `missing top chrome text: ${expected}`);
  }
  const clippedTopNav = await page.evaluate(() => {
    const header = document.querySelector("header");
    if (!header) return ["missing header"];
    const labels = ["Project", "Workspace", "Agent", "Media", "Review", "Deliver", "Settings"];
    const clipped = [];
    for (const label of labels) {
      const button = Array.from(header.querySelectorAll("button")).find(
        (el) => el.textContent?.trim() === label,
      );
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

await check("workflow lens row exposes all 9 lenses", async () => {
  const { page } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
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
    assert.ok(lensNames.some((s) => s.includes(expected)), `lens "${expected}" missing`);
  }
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
