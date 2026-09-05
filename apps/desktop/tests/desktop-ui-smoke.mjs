#!/usr/bin/env node
/** Smoke the real application with the shared Tauri IPC fixture. */

import { chromium } from "playwright";
import { strict as assert } from "node:assert";
import { spawn } from "node:child_process";
import { mkdirSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";

const BASE_URL = process.env.SMOKE_URL ?? "http://127.0.0.1:1420/";
const APP_URL = new URL(BASE_URL);
const APP_PORT = APP_URL.port || (APP_URL.protocol === "https:" ? "443" : "80");
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

  appServer = spawn("pnpm", ["--dir", "apps/desktop", "exec", "vite", "--host", "127.0.0.1", "--port", APP_PORT, "--strictPort"], {
    cwd: new URL("../../..", import.meta.url),
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

let browser;
try {
  browser = await chromium.launch();
} catch (error) {
  await stopAppServer();
  throw error;
}

// Pre-seed the first-run consent flag so `WelcomeCard` stays dismissed.
// Without this every smoke page boots with the welcome modal covering
// the screen, so text assertions miss and clicks hit the backdrop.
const SUPPRESS_WELCOME = `
  try {
    localStorage.setItem("montage:welcome:consent", new Date().toISOString());
  } catch {}
`;

const ctx = await browser.newContext({
  viewport: { width: 1586, height: 992 },
  deviceScaleFactor: 1,
});
await ctx.addInitScript(SUPPRESS_WELCOME);

const passes = [];
const failures = [];
let activePage;
let activeErrors;

async function check(name, fn) {
  try {
    await fn();
    passes.push(name);
    console.log(`  ok  ${name}`);
  } catch (err) {
    failures.push({ name, err });
    if (activePage && !activePage.isClosed()) {
      console.error("Page errors:", activeErrors);
      console.error("Page content:", (await activePage.locator("body").innerText()).slice(0, 3000));
      await activePage.screenshot({ path: `${SCREENSHOT_DIR}/failure-${failures.length}.png` });
    }
    console.error(`  FAIL  ${name}\n    ${err.message}`);
  }
}

async function makePage() {
  const page = await ctx.newPage();
  const errors = [];
  activePage = page;
  activeErrors = errors;
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.stack ?? e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(`console.error: ${m.text()}`);
  });
  return { page, errors };
}

try {
  await check("no project shows the real landing page", async () => {
    const { page, errors } = await makePage();
    await page.goto(new URL("tests/ui-harness.html", BASE_URL).href);
    await page.getByRole("button", { name: /^New Project/ }).waitFor();
    assert.equal(await page.locator(".stage-left-pane:visible").count(), 0);
    assert.deepEqual(errors, []);
    await page.close();
  });

  await check("project workspace uses the real panes and timeline", async () => {
    const { page, errors } = await makePage();
    await page.goto(new URL("tests/ui-harness.html?project=1", BASE_URL).href);
    await page.getByRole("button", { name: "Chat", exact: true }).waitFor();
    await page.getByRole("button", { name: "Media", exact: true }).click();
    await page.locator(".stage-left-pane").waitFor();
    await page.getByRole("button", { name: "Index", exact: true }).click();
    await page.locator(".index-rail").waitFor();
    await page.evaluate(() => window.dispatchEvent(
      new CustomEvent("montage-menu-command", { detail: "view:transcript" }),
    ));
    await page.locator('.stage-left-pane button[data-active="true"]').filter({ hasText: "Transcript" }).waitFor();
    await page.getByRole("button", { name: "Inspector", exact: true }).click();
    await page.locator('.stage-right-pane button[data-active="true"]').filter({ hasText: "Inspector" }).waitFor();
    await page.getByRole("button", { name: "Chat", exact: true }).click();
    await page.locator('.stage-right-pane button[data-active="true"]').filter({ hasText: "Chat" }).waitFor();
    assert.ok(await page.locator("canvas").count() > 0, "timeline canvas is mounted");
    await page.screenshot({ path: `${SCREENSHOT_DIR}/workspace.png` });
    assert.deepEqual(errors, []);
    await page.close();
  });
  await check("proposal queue uses the actual trim boundary", async () => {
    const { page, errors } = await makePage();
    await page.goto(new URL("tests/ui-harness.html?project=1&scenario=proposal", BASE_URL).href);
    const change = page.getByRole("button", { name: /Change 1/ });
    await change.waitFor();
    assert.match(await change.innerText(), /0:07/);
    await change.click();
    assert.deepEqual(errors, []);
    await page.close();
  });

  await check("moved-clip review seeks the current clip, not its proposed destination", async () => {
    const { page, errors } = await makePage();
    await page.goto(new URL("tests/ui-harness.html?project=1&scenario=proposal", BASE_URL).href);
    await page.getByRole("button", { name: /Change 1/ }).waitFor();
    await page.evaluate(async () => {
      const { useProposalStore } = await import("/src/timeline/proposal.ts");
      const active = useProposalStore.getState().active;
      const snapshot = structuredClone(active.snapshot);
      const items = snapshot.tracks[0].items;
      const moved = items.find((item) => item.kind === "clip" && item.clip_uuid === "clip-2");
      snapshot.tracks[0].items = [
        { ...moved, index: 0, track_start_s: 0 },
        ...items.filter((item) => item !== moved).map((item, i) => ({
          ...item, index: i + 1, track_start_s: item.track_start_s + moved.duration_s,
        })),
      ];
      const next = { ...active, snapshot, diffHints: [{ kind: "move", op_index: 0,
        from_track_index: 0, from_item_index: 2, to_track_index: 0, to_item_index: 0 }] };
      useProposalStore.setState({ active: next, pending: [next] });
    });
    const change = page.getByRole("button", { name: /Change 1/ });
    await page.waitForFunction(() => [...document.querySelectorAll("button")]
      .some((button) => button.textContent.includes("Change 1") && button.textContent.includes("0:06")));
    await change.click();
    await page.waitForFunction(async () => {
      const { useMediaStore } = await import("/src/media/store.ts");
      return Math.abs(useMediaStore.getState().timelineTime - 6) < 0.1;
    });
    assert.deepEqual(errors, []);
    await page.close();
  });


  await check("delivery, skills, and history use the live workspace", async () => {
    const { page, errors } = await makePage();
    await page.goto(new URL("tests/ui-harness.html?project=1", BASE_URL).href);
    await page.getByRole("button", { name: "Chat", exact: true }).waitFor();
    for (const stage of ["deliver", "skills", "history"]) {
      await page.evaluate(async (next) => {
        const { useStageStore } = await import("/src/state/stages.ts");
        useStageStore.getState().set(next);
      }, stage);
      await page.getByRole("button", { name: "← Stage", exact: true }).waitFor();
      await page.locator("span.capitalize").filter({ hasText: new RegExp(`^${stage}$`) }).waitFor();
      if (stage === "skills") await page.getByText("Podcast edit", { exact: true }).waitFor();
      assert.equal(await page.getByText("Delivery confidence", { exact: true }).count(), 0);
      await page.screenshot({ path: `${SCREENSHOT_DIR}/${stage}.png` });
    }
    assert.deepEqual(errors, []);
    await page.close();
  });

} finally {
  await browser.close();
  await stopAppServer();
}
console.log(`\n${passes.length} passed, ${failures.length} failed`);
process.exitCode = failures.length > 0 ? 1 : 0;
