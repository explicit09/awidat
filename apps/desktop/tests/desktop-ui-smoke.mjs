#!/usr/bin/env node
/**
 * Desktop UI smoke test.
 *
 * Runs the browser/static app path used for design review. With no Tauri
 * project open, the app should boot directly into the Screen 2 golden cockpit:
 * app chrome, workflow lenses, agent command rail, proposal preview, timeline,
 * and proposal inspector.
 *
 * If nothing is already serving `BASE_URL`, this script spawns `vite preview`
 * against the built `dist/` directory and tears it down on exit. The build
 * step in CI runs before this script, so `dist/` is present.
 */

import { chromium } from "playwright";
import { strict as assert } from "node:assert";
import { mkdirSync } from "node:fs";
import { spawn } from "node:child_process";
import { connect } from "node:net";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const BASE_URL = process.env.SMOKE_URL ?? "http://localhost:1420/";
const SCREENSHOT_DIR = process.env.SMOKE_OUT_DIR ?? "tests/smoke";
mkdirSync(SCREENSHOT_DIR, { recursive: true });

function probePort(port, host = "127.0.0.1", timeoutMs = 250) {
  return new Promise((resolvePromise) => {
    const sock = connect({ port, host });
    const done = (ok) => {
      sock.destroy();
      resolvePromise(ok);
    };
    sock.once("connect", () => done(true));
    sock.once("error", () => done(false));
    setTimeout(() => done(false), timeoutMs);
  });
}

async function waitForPort(port, host, deadlineMs) {
  const stop = Date.now() + deadlineMs;
  while (Date.now() < stop) {
    if (await probePort(port, host)) return;
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`timed out waiting for ${host}:${port}`);
}

const baseUrlObj = new URL(BASE_URL);
const baseHost = baseUrlObj.hostname || "127.0.0.1";
const basePort = Number(baseUrlObj.port) || (baseUrlObj.protocol === "https:" ? 443 : 80);

let spawnedServer = null;
function killSpawnedServer() {
  if (spawnedServer && spawnedServer.exitCode === null) {
    spawnedServer.kill("SIGTERM");
  }
}

if (!(await probePort(basePort, baseHost))) {
  const desktopDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  spawnedServer = spawn(
    "pnpm",
    ["exec", "vite", "preview", "--host", baseHost, "--port", String(basePort), "--strictPort"],
    { cwd: desktopDir, stdio: ["ignore", "inherit", "inherit"] },
  );
  process.on("exit", killSpawnedServer);
  process.on("SIGINT", () => { killSpawnedServer(); process.exit(130); });
  process.on("SIGTERM", () => { killSpawnedServer(); process.exit(143); });
  try {
    await waitForPort(basePort, baseHost, 30_000);
  } catch (err) {
    killSpawnedServer();
    console.error(`failed to bring up vite preview: ${err.message}`);
    process.exit(1);
  }
}

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
      "Agent Command",
      "Podcast Tightening v1",
      "Proposal Inspector",
      "Timeline",
      "Transcript",
      "Evidence",
    ],
  },
  {
    id: "screen3",
    text: ["Agent command history", "Proposed changes", "The #1 thing that kills startups", "Before (Source)", "After (Proposed Clip)", "Batch insights", "Revise with prompt"],
  },
  {
    id: "screen4",
    text: ["Selected sentence", "Review · Transcript", "What this cut does", "Pending trim", "Keep this pause", "Edit around selection"],
  },
  {
    id: "screen5",
    text: ["CUT 12 · L-cut", "Current timeline", "Proposed timeline", "Render output context", "Compare alternatives", "Inspect deeper"],
  },
  {
    id: "screen6",
    text: ["Import files", "Import URL", "Indexing pipeline", "Transcripts", "Speaker diarization", "Indexing in progress", "Ask agent for first cut"],
  },
  {
    id: "screen7",
    text: ["Targets", "YouTube", "TikTok", "Preflight", "Render summary", "Delivery confidence", "Export now"],
  },
  {
    id: "screen8",
    text: ["No media imported", "Indexing media", "Review transitions", "Proposal generation is blocked", "Repair with agent"],
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
    "Intent",
    "Indexing",
    "Proposal",
    "Review",
    "Revise",
    "Deliver",
    "Interview_A",
    "Podcast Tightening v1",
  ]) {
    assert.ok(body.includes(expected), `missing top chrome text: ${expected}`);
  }
  for (const label of ["Share", "Settings"]) {
    assert.equal(await page.locator(`header button[aria-label="${label}"]`).count(), 1, `missing top chrome action: ${label}`);
  }
  const clippedTopNav = await page.evaluate(() => {
    const header = document.querySelector("header");
    if (!header) return ["missing header"];
    const labels = ["Intent", "Indexing", "Proposal", "Review", "Revise", "Deliver"];
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
killSpawnedServer();
if (failures.length > 0) {
  process.exit(1);
}
