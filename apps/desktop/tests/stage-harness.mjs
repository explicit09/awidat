#!/usr/bin/env node
/**
 * Stage harness screenshot gate (Task 8).
 *
 * Boots (or reuses) the desktop dev server, loads the deterministic
 * `/stage-harness` route from Task 7 at a frozen clock time, waits for
 * `document.title === "stage-harness-ready"`, screenshots just the
 * program-frame element, and compares it against a committed
 * per-platform golden via ffmpeg SSIM (see `scripts/ssim-compare.sh`).
 *
 * First run for a given platform has no golden yet: it writes the
 * screenshot AS the golden and prints "golden bootstrapped" so CI/dev
 * can self-seed once, then gate on every run after.
 *
 * Follows the dev-server boot/reuse pattern of `tests/desktop-ui-smoke.mjs`
 * (canReachApp / ensureAppServer / stopAppServer), spawning
 * `pnpm --dir apps/desktop exec vite` when the app isn't already up.
 */

import { chromium } from "playwright";
import { strict as assert } from "node:assert";
import { spawn, execFileSync } from "node:child_process";
import { mkdirSync, existsSync, copyFileSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = fileURLToPath(new URL("../../..", import.meta.url));
const BASE_URL = process.env.SMOKE_URL ?? "http://127.0.0.1:1420/";
const APP_URL = new URL(BASE_URL);
const APP_PORT = APP_URL.port || (APP_URL.protocol === "https:" ? "443" : "80");
const SCREENSHOT_DIR = process.env.SMOKE_OUT_DIR ?? "tests/smoke";
const GOLDEN_DIR = "tests/fixtures/stage-golden";
mkdirSync(SCREENSHOT_DIR, { recursive: true });
mkdirSync(GOLDEN_DIR, { recursive: true });

const HARNESS_T = "1.0";
const HARNESS_SCENE = "/fixtures/stage/scene-basic.json";
const HARNESS_URL = new URL(`/stage-harness?t=${HARNESS_T}&scene=${HARNESS_SCENE}`, BASE_URL).toString();

const SHOT_NAME = `stage-harness-t${HARNESS_T}.png`;
const SHOT_PATH = path.join(SCREENSHOT_DIR, SHOT_NAME);
const GOLDEN_NAME = `scene-basic-t${HARNESS_T}-${process.platform}.png`;
const GOLDEN_PATH = path.join(GOLDEN_DIR, GOLDEN_NAME);
const MIN_SSIM = "0.98";

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

  appServer = spawn("pnpm", ["--dir", "apps/desktop", "exec", "vite", "--host", "127.0.0.1", "--port", APP_PORT, "--strictPort"], {
    cwd: REPO_ROOT,
    env: { ...process.env, BROWSER: "none", VITE_MONTAGE_SKIP_WELCOME: "1" },
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
const ctx = await browser.newContext({
  viewport: { width: 1280, height: 720 },
  deviceScaleFactor: 1,
});

let exitCode = 0;

try {
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(`console.error: ${m.text()}`);
  });

  await page.goto(HARNESS_URL, { waitUntil: "networkidle" });
  await page.waitForFunction(() => document.title === "stage-harness-ready", null, { timeout: 15000 });

  assert.deepEqual(errors, [], `harness console/page errors: ${errors.join("; ")}`);

  // Assert the expected overlay DOM landed before trusting the screenshot:
  // one title text node, one shape rect, one image.
  const frame = page.locator('[data-testid="stage-harness-root"]');
  await frame.waitFor({ state: "visible" });

  const titleLayer = page.locator(".timeline-title-layer .timeline-title-overlay");
  assert.equal(await titleLayer.count(), 1, "expected exactly one title overlay");
  const titleText = (await titleLayer.first().textContent()) ?? "";
  assert.ok(titleText.includes("STAGE HARNESS"), `title overlay text mismatch: ${titleText}`);

  const shapeLayer = page.locator(".timeline-motion-shape-rect");
  assert.equal(await shapeLayer.count(), 1, "expected exactly one shape overlay");

  const imageLayer = page.locator(".timeline-motion-image");
  assert.equal(await imageLayer.count(), 1, "expected exactly one image overlay");

  // Screenshot only the program-frame element — avoids window-chrome
  // variance since the frame itself is a fixed 1280x720 box.
  await frame.screenshot({ path: SHOT_PATH });

  await page.close();

  if (!existsSync(GOLDEN_PATH)) {
    copyFileSync(SHOT_PATH, GOLDEN_PATH);
    console.log(`golden bootstrapped: ${GOLDEN_PATH}`);
  } else {
    try {
      execFileSync(
        path.join(REPO_ROOT, "scripts/ssim-compare.sh"),
        [SHOT_PATH, GOLDEN_PATH, MIN_SSIM],
        { stdio: "inherit" },
      );
      console.log(`stage-harness screenshot matches golden (>= ${MIN_SSIM})`);
    } catch (err) {
      console.error(`stage-harness screenshot diverged from golden below ${MIN_SSIM}`);
      exitCode = 1;
    }
  }
} catch (err) {
  console.error(`FAIL stage-harness: ${err.message}`);
  exitCode = 1;
} finally {
  await browser.close();
  await stopAppServer();
}

process.exit(exitCode);
