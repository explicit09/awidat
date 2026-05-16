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

    await page.goto(`${baseUrl}/tests/ui-harness.html?project=1&scenario=assessor`);
    await page.getByText("assess_edit_quality").waitFor({ state: "visible", timeout: 5_000 });
    await page.locator(".tool-summary-row").click();
    await expectVisible(page, ".tool-recommendation", "assessor recommendation action");
    await page.getByRole("button", { name: "Propose" }).click();
    await expectVisible(page, ".proposal-actions", "assessor-generated proposal toolbar");
    const assessorEdl = await page.evaluate(() => window.__lastEdlText);
    if (
      !assessorEdl.includes("*** Set Audio Lead") ||
      !assessorEdl.includes("@@ anchor: clip_uuid=clip-2") ||
      !assessorEdl.includes("+ lead_s: 0.350") ||
      !assessorEdl.includes("*** Set Cut Intent") ||
      !assessorEdl.includes("+ cut_type: j_cut")
    ) {
      throw new Error(`assessor recommendation did not emit expected EDL: ${assessorEdl}`);
    }
    await page.getByRole("button", { name: "Reject" }).click();
    await page.locator(".proposal-actions").waitFor({ state: "hidden", timeout: 5_000 });
    await page.evaluate(() =>
      window.__seedAssessorRecommendation({
        id: "assess-edit-quality-follow-up",
        action: "use_l_cut",
        cutType: "l_cut",
        intent: "hold_reaction",
        edlOp: "Set Audio Trail",
        reason: "Hold the outgoing line under the reaction after rejecting the first J-cut.",
        edlGuidance: "*** Set Audio Trail with trail_s 0.45, then review by ear",
      }),
    );
    await page.locator(".tool-summary-row").last().click();
    await page.locator(".tool-recommendation").last().getByRole("button", { name: "Propose" }).click();
    await expectVisible(page, ".proposal-actions", "follow-up assessor proposal toolbar");
    const followUpEdl = await page.evaluate(() => window.__lastEdlText);
    if (
      !followUpEdl.includes("*** Set Audio Trail") ||
      !followUpEdl.includes("@@ anchor: clip_uuid=clip-1") ||
      !followUpEdl.includes("+ trail_s: 0.450") ||
      !followUpEdl.includes("+ cut_type: l_cut")
    ) {
      throw new Error(`follow-up assessor recommendation did not emit expected EDL: ${followUpEdl}`);
    }

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

    await page.goto(`${baseUrl}/tests/ui-harness.html?project=1&scenario=editorial`);
    await page.getByText("Selected clip").waitFor({ state: "visible", timeout: 5_000 });
    await expectVisible(page, ".media-preview-limits", "preview limitation banner");
    await page
      .getByText("Split-edit offsets are shown in the timeline but approximated in preview audio.")
      .waitFor({ state: "visible", timeout: 5_000 });
    await page
      .locator(".properties-intent-chip")
      .filter({ hasText: /^Cut On Action$/ })
      .waitFor({ state: "visible", timeout: 5_000 });
    await page
      .locator(".properties-intent-chip")
      .filter({ hasText: /^Cutaway$/ })
      .waitFor({ state: "visible", timeout: 5_000 });
    await page
      .getByText("Hold dialogue under the incoming reaction.")
      .waitFor({ state: "visible", timeout: 5_000 });
    await expectVisible(page, ".timeline-cut-badge", "timeline cut badge");
    await expectVisible(page, ".timeline-split-offset", "timeline split-edit offset");
    await expectVisible(page, ".properties-cut-type-select", "cut intent editor");
    await page.locator(".properties-cut-type-select").first().selectOption("hard_cut");
    await page.getByRole("button", { name: "Apply cut intent" }).first().click();
    const cutIntentEdl = await page.evaluate(() => window.__lastEdlText);
    if (
      !cutIntentEdl.includes("*** Set Cut Intent") ||
      !cutIntentEdl.includes("@@ between: clip_uuid=clip-1 and clip_uuid=clip-2") ||
      !cutIntentEdl.includes("+ cut_type: hard_cut")
    ) {
      throw new Error(`cut intent editor did not emit expected EDL: ${cutIntentEdl}`);
    }
    await page.getByRole("button", { name: "Use J-cut" }).first().click();
    const jCutEdl = await page.evaluate(() => window.__lastEdlText);
    if (
      !jCutEdl.includes("*** Set Audio Lead") ||
      !jCutEdl.includes("@@ anchor: clip_uuid=clip-2") ||
      !jCutEdl.includes("+ lead_s: 0.350")
    ) {
      throw new Error(`J-cut alternative did not emit expected EDL: ${jCutEdl}`);
    }

    await page.goto(`${baseUrl}/tests/ui-harness.html?project=1&scenario=density`);
    await page.getByText("Selected transition").waitFor({ state: "visible", timeout: 5_000 });
    await page
      .getByText("3 visible transitions land within this 30s window.")
      .waitFor({ state: "visible", timeout: 5_000 });

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
