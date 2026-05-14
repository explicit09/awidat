#!/usr/bin/env node
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { readFileSync } from "node:fs";

const port = Number(process.env.AWIDAT_DESKTOP_TEST_PORT || 1430);
const baseUrl = `http://127.0.0.1:${port}`;
const desktopRoot = fileURLToPath(new URL("..", import.meta.url));
const repoRoot = fileURLToPath(new URL("../../..", import.meta.url));

const builtInTransitionIds = [
  "awidat.cross_dissolve",
  "awidat.fade_black",
  "awidat.flash_white",
  "awidat.wipe_left",
  "awidat.wipe_right",
  "awidat.slide_left",
  "awidat.slide_right",
  "awidat.smooth_push_left",
  "awidat.zoom_in",
  "awidat.pixelize",
  "awidat.radial",
];

function assertDesktopTransitionCoverage() {
  const previewSource = readFileSync(
    `${desktopRoot}/src/media/SegmentedVideoView.tsx`,
    "utf8",
  );
  const propertiesSource = readFileSync(
    `${desktopRoot}/src/properties/PropertiesPane.tsx`,
    "utf8",
  );
  const timelineSource = readFileSync(
    `${desktopRoot}/src/timeline/TimelinePane.tsx`,
    "utf8",
  );
  const registrySource = readFileSync(
    `${repoRoot}/crates/proto/src/transitions.rs`,
    "utf8",
  );
  for (const id of builtInTransitionIds) {
    if (!registrySource.includes(`id: "${id}"`)) {
      throw new Error(`test fixture transition id is not in proto registry: ${id}`);
    }
    if (!propertiesSource.includes(`value: "${id}"`)) {
      throw new Error(`transition inspector option missing for ${id}`);
    }
    if (!previewSource.includes(`"${id}"`)) {
      throw new Error(`timeline preview handling missing for ${id}`);
    }
    if (!timelineSource.includes(`"${id}"`)) {
      throw new Error(`timeline label handling missing for ${id}`);
    }
  }
}

function startVite() {
  const child = spawn(
    "pnpm",
    ["exec", "vite", "--host", "127.0.0.1", "--port", String(port), "--strictPort"],
    {
      cwd: desktopRoot,
      stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, CI: "1" },
    },
  );
  child.stdout.on("data", (chunk) => process.stdout.write(chunk));
  child.stderr.on("data", (chunk) => process.stderr.write(chunk));
  return child;
}

async function waitForServer(child) {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`vite exited early with ${child.exitCode}`);
    }
    try {
      const res = await fetch(baseUrl);
      if (res.ok) return;
    } catch {
      // keep polling
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`timed out waiting for ${baseUrl}`);
}

async function expectVisible(page, selector, label) {
  const locator = page.locator(selector);
  await locator.first().waitFor({ state: "visible", timeout: 5_000 });
  const count = await locator.count();
  if (count < 1) throw new Error(`${label} not found`);
}

async function run() {
  assertDesktopTransitionCoverage();
  const server = startVite();
  let browser;
  try {
    await waitForServer(server);
    browser = await chromium.launch();
    const page = await browser.newPage({ viewport: { width: 1280, height: 820 } });
    page.on("console", (msg) => console.log(`[browser:${msg.type()}] ${msg.text()}`));
    page.on("pageerror", (error) => console.error(`[browser:error] ${error.message}`));

    await page.goto(`${baseUrl}/tests/ui-harness.html`);
    await expectVisible(page, ".app-header", "app shell header");
    await expectVisible(page, ".project-launcher", "empty project launcher");

    await page.goto(`${baseUrl}/tests/ui-harness.html?project=1`);
    await expectVisible(page, ".workspace-editor", "project workspace");
    await expectVisible(page, ".timeline-pane", "timeline pane");
    await expectVisible(page, ".note-card-broll", "b-roll note card");
    await expectVisible(page, ".note-broll-use", "b-roll Use this button");

    await page.goto(`${baseUrl}/tests/ui-harness.html?project=1&scenario=proposal`);
    await expectVisible(page, ".proposal-actions", "proposal action toolbar");

    await page.goto(`${baseUrl}/tests/ui-harness.html?project=1&scenario=transition`);
    await page.getByText("Selected transition").waitFor({ state: "visible", timeout: 5_000 });
    await expectVisible(page, ".properties-select", "transition kind selector");
    const selectedKind = await page.locator(".properties-select").inputValue();
    if (selectedKind !== "awidat.cross_dissolve") {
      throw new Error(`unexpected selected transition kind: ${selectedKind}`);
    }
    await page.locator(".properties-select").selectOption("awidat.fade_black");
    await page.locator(".properties-number-input").first().fill("0.6");
    await page.getByRole("button", { name: "Apply" }).click();
    const applyEdl = await page.evaluate(() => window.__lastEdlText);
    if (
      !applyEdl.includes("*** Insert Transition") ||
      !applyEdl.includes("+ kind: awidat.fade_black") ||
      !applyEdl.includes("+ duration_s: 0.600")
    ) {
      throw new Error(`transition Apply did not emit expected EDL: ${applyEdl}`);
    }
    await page.getByRole("button", { name: "Delete" }).click();
    const deleteEdl = await page.evaluate(() => window.__lastEdlText);
    if (!deleteEdl.includes("*** Delete Transition")) {
      throw new Error(`transition Delete did not emit expected EDL: ${deleteEdl}`);
    }

    await page.getByRole("tab", { name: "Vedit" }).click();
    await expectVisible(page, ".vedit-panel", "vedit panel");
    await page.locator(".vedit-entry").filter({ hasText: "Clean starting cut" }).locator("summary").click();
    await page.getByRole("button", { name: "Inspect diff" }).click();
    await expectVisible(page, ".vedit-diff", "vedit diff preview");
    await page.getByRole("button", { name: "Restore this cut" }).click();
    await expectVisible(page, ".vedit-restore-confirm", "vedit restore confirmation");

    console.log("desktop UI smoke passed");
  } finally {
    if (browser) await browser.close();
    server.kill("SIGTERM");
  }
}

run().catch((error) => {
  console.error(error);
  process.exit(1);
});
