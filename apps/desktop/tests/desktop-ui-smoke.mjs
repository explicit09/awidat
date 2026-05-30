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
import { spawn } from "node:child_process";
import { mkdirSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";

const BASE_URL = process.env.SMOKE_URL ?? "http://localhost:1420/";
const SCREENSHOT_DIR = process.env.SMOKE_OUT_DIR ?? "tests/smoke";
mkdirSync(SCREENSHOT_DIR, { recursive: true });

let appServer = null;

async function canReachApp() {
  try {
    const response = await fetch(BASE_URL, { signal: AbortSignal.timeout(500) });
    return response.ok;
  } catch {
    return false;
  }
}

async function ensureAppServer() {
  if (await canReachApp()) return;

  appServer = spawn("pnpm", ["--dir", "apps/desktop", "exec", "vite", "--host", "127.0.0.1"], {
    cwd: new URL("../../..", import.meta.url),
    env: { ...process.env, BROWSER: "none" },
    detached: process.platform !== "win32",
    stdio: ["ignore", "pipe", "pipe"],
  });

  appServer.stdout.on("data", (chunk) => process.stdout.write(chunk));
  appServer.stderr.on("data", (chunk) => process.stderr.write(chunk));

  for (let attempt = 0; attempt < 60; attempt += 1) {
    if (await canReachApp()) return;
    if (appServer.exitCode !== null) {
      throw new Error(`dev server exited before ${BASE_URL} became reachable`);
    }
    await delay(500);
  }

  throw new Error(`timed out waiting for ${BASE_URL}`);
}

async function stopAppServer() {
  if (appServer && appServer.exitCode === null) {
    const stopped = new Promise((resolve) => {
      appServer.once("exit", resolve);
    });
    if (process.platform === "win32") {
      appServer.kill("SIGTERM");
    } else {
      process.kill(-appServer.pid, "SIGTERM");
    }
    await Promise.race([stopped, delay(2000)]);
  }
}

process.on("SIGINT", () => {
  stopAppServer();
  process.exit(130);
});
process.on("SIGTERM", () => {
  stopAppServer();
  process.exit(143);
});

await ensureAppServer();

const browser = await chromium.launch();

// Pre-seed the first-run welcome flag so `WelcomeCard` stays dismissed.
// Without this every smoke page boots with the welcome modal covering
// the screen, so text assertions miss and clicks hit the backdrop.
const SUPPRESS_WELCOME = `
  try {
    localStorage.setItem("awidat:welcome:shown", new Date().toISOString());
  } catch {}
`;

const ctx = await browser.newContext({
  viewport: { width: 1586, height: 992 },
  deviceScaleFactor: 1,
});
await ctx.addInitScript(SUPPRESS_WELCOME);

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
  // Each screen's text checks should target durable content the
  // redesigned UI actually renders, not the spec doc titles. When a
  // demo redesigns its content, update these to track what's on screen.
  {
    id: "screen2",
    url: BASE_URL,
    text: [
      "Agent",
      "Suggested next actions",
      "Review rough cut",
      "Proposal Inspector",
      "Plan",
    ],
  },
  {
    id: "screen4",
    text: ["Selected sentence", "Speaker", "Confidence", "Pending", "Accepted", "Evidence for selection"],
  },
  {
    id: "screen5",
    text: ["Cut 12 · L-cut", "Selected change region", "Current timeline", "Proposed timeline", "Proposal Inspector"],
  },
  {
    id: "screen6",
    text: ["Index readiness", "Transcript", "Speaker", "Captions", "Scenes"],
  },
  {
    id: "screen7",
    text: ["Targets", "YouTube", "TikTok", "Preflight", "Caption safe area"],
  },
  {
    id: "screen8",
    text: ["Empty state", "No project open yet", "Import media", "Open project", "Start with example"],
  },
  {
    id: "screen9",
    text: ["Proposal cards", "Cut 07 · J-cut", "pending", "accepted"],
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
  await compact.addInitScript(SUPPRESS_WELCOME);
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
    // Brand mark is uppercase in the redesigned IdentityRow. The
    // legacy "Main Desktop Workspace" banner was removed when the
    // top chrome became the WorkspaceRow tablist — Edit/Deliver are
    // the durable navigation labels now.
    "AWIDAT",
    "Edit",
    "Deliver",
  ]) {
    assert.ok(body.includes(expected), `missing top chrome text: ${expected}`);
  }
  // Settings button uses aria-label (not title) in the redesigned IdentityRow.
  await page.getByRole("button", { name: "Settings" }).waitFor({ state: "visible" });
  const clippedTopNav = await page.evaluate(() => {
    // The redesigned chrome is the WorkspaceRow tablist (no <header> wrapper).
    const tablist = document.querySelector('[role="tablist"][aria-label="Workspace"]');
    if (!tablist) return ["missing workspace tablist"];
    const labels = ["Edit", "Deliver"];
    const clipped = [];
    const buttons = Array.from(tablist.querySelectorAll('button[role="tab"]'));
    for (const label of labels) {
      const button = buttons.find((el) => el.textContent?.trim() === label);
      if (!button) {
        clipped.push(`${label}: missing`);
        continue;
      }
      const rect = button.getBoundingClientRect();
      let node = button.parentElement;
      while (node && node !== document.body) {
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

await check("top workspace row is the only global workflow navigation", async () => {
  const { page } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  const lensRows = await page.locator('[role="tablist"][aria-label="Workflow lens"]').count();
  assert.equal(lensRows, 0, "secondary workflow lens row should not render");
  // The legacy Stage tablist was replaced by the WorkspaceRow.
  const tabNames = await page
    .locator('[role="tablist"][aria-label="Workspace"] button[role="tab"]')
    .allTextContents();
  for (const expected of ["Edit", "Deliver"]) {
    assert.ok(tabNames.some((s) => s.includes(expected)), `workspace tab "${expected}" missing`);
  }
  for (const folded of ["Intent", "Index", "Proposal", "Review", "Revise", "Import", "Selects", "Assembly", "Captions", "Audio", "Color"]) {
    assert.ok(!tabNames.some((s) => s.includes(folded)), `${folded} should not be a second-level global tab`);
  }
  await page.close();
});

await check("workspace row renders the live preview timecode", async () => {
  const { page } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  const tablist = page.locator('[role="tablist"][aria-label="Workspace"]');
  await tablist.waitFor({ state: "visible" });
  // Timecode lives inside the WorkspaceRow tablist (sibling of the tabs)
  // and is rendered as HH:MM:SS:FF / HH:MM:SS:FF at 24 fps.
  const rowText = (await tablist.textContent()) ?? "";
  assert.ok(
    /\d{2}:\d{2}:\d{2}:\d{2}\s*\/\s*\d{2}:\d{2}:\d{2}:\d{2}/.test(rowText),
    `expected HH:MM:SS:FF / HH:MM:SS:FF timecode in workspace row, got: ${rowText.trim()}`,
  );
  await page.close();
});

await check("indexing dashboard surfaces pipeline readiness", async () => {
  const { page } = await makePage();
  await page.goto(demoUrl("screen6"), { waitUntil: "networkidle" });
  const body = await page.textContent("body");
  // Wave 5 replaced the "Project & import" / "Project media" dashboard
  // with an indexer-readiness pipeline showing per-stage progress.
  for (const expected of [
    "Index readiness",
    "Indexing",
    "Transcript",
    "Speaker",
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

await check("indexing dashboard breaks readiness down by sub-pipeline", async () => {
  const { page } = await makePage();
  await page.goto(demoUrl("screen6"), { waitUntil: "networkidle" });
  const body = await page.textContent("body");
  // The dashboard groups Speech / Visuals / Audio with per-indexer
  // status lines (Scenes, Face, Motion, Color, Silence, Captions).
  for (const expected of [
    "Speech",
    "Visuals",
    "Audio",
    "Scenes",
    "Captions",
    "Silence",
  ]) {
    assert.ok(body.includes(expected), `missing indexing sub-pipeline text: ${expected}`);
  }
  await page.close();
});

await check("indexing dashboard shows duration and structural counts", async () => {
  const { page } = await makePage();
  await page.goto(demoUrl("screen6"), { waitUntil: "networkidle" });
  const body = await page.textContent("body");
  // Structural readout: total Duration, Scenes count, Segments count.
  for (const expected of [
    "Duration",
    "Scenes",
    "Segments",
  ]) {
    assert.ok(body.includes(expected), `missing structural readout: ${expected}`);
  }
  await page.close();
});

await check("agent command rail renders Screen 2 intent, context, plan, activity, suggestions", async () => {
  const { page } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  const body = (await page.textContent("body")).toLowerCase();
  for (const expected of [
    "Manual",
    "Cut this into a tight 8-minute podcast episode.",
    "Remove dead air but preserve natural pacing.",
    "Clip Interview_A",
    "Range: 00:12-18:40",
    "Transcript region selected",
    "Target: YouTube 16:9",
    "Running",
    "Build assembly (rough cut)",
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
    "12 pending",
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

await check("bottom dock is timeline-only — no transcript or vedit tabs", async () => {
  const { page } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  const body = (await page.textContent("body")).toLowerCase();
  assert.ok(body.includes("timeline"), "timeline label should be present");
  // Transcript + Vedit moved to the right rail; the old dock header is gone.
  assert.equal(await page.locator(".edit-dock-header").count(), 0, "edit dock header should be removed");
  assert.equal(await page.locator(".edit-lower-dock").count(), 0, "edit-lower-dock wrapper should be removed");
  await page.close();
});

await check("right rail exposes transcript and Vedit alongside Inspector and Index", async () => {
  const { page, errors } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  await page.getByRole("button", { name: "Transcript", exact: true }).click();
  let body = (await page.textContent("body")).toLowerCase();
  assert.ok(
    body.includes("no transcript") || body.includes("loading transcript") || body.includes("segments"),
    "transcript panel should be reachable from the right rail",
  );
  await page.getByRole("button", { name: "Vedit", exact: true }).click();
  body = (await page.textContent("body")).toLowerCase();
  assert.ok(
    body.includes("timeline history") || body.includes("no vedit commits yet"),
    "Vedit panel should be reachable from the right rail",
  );
  assert.deepEqual(errors, []);
  await page.close();
});

await check("footer exposes status, autosave, render, and disk", async () => {
  const { page } = await makePage();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  // The footer was rebuilt as a compact status strip: indexer-state
  // pill (ready) on the left; autosave + render + disk on the right.
  // We assert against the live DOM rather than the legacy copy.
  const footer = (await page.locator("footer").textContent()) ?? "";
  for (const expected of [
    "ready",
    "Autosaved",
    "render",
    "disk",
  ]) {
    assert.ok(footer.includes(expected), `missing footer text: ${expected} (got: ${footer.trim()})`);
  }
  await page.close();
});

await browser.close();
await stopAppServer();

console.log(`\n${passes.length} passed, ${failures.length} failed`);
process.exit(failures.length > 0 ? 1 : 0);
