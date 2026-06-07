#!/usr/bin/env node
/**
 * Performance budget — measures load + paint + render on the new shell
 * and fails if budgets are exceeded.
 *
 * Budgets are conservative starting points; tighten as the app stabilizes.
 * Run with `pnpm dev` on :1420.
 */

import { chromium } from "playwright";
import { spawn } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";

const BASE_URL = process.env.PERF_URL ?? "http://localhost:1420/";

const BUDGETS = {
  loadMs: 3000,
  fcpMs: 2500,
  domNodes: 2000,
  jsHeapMB: 80,
  workspaceSwitchMs: 700,
  projectShellMs: 1200,
  projectWorkspaceSwitchMs: 750,
};

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

process.on("SIGINT", async () => {
  await stopAppServer();
  process.exit(130);
});
process.on("SIGTERM", async () => {
  await stopAppServer();
  process.exit(143);
});

let browser = null;
let loadMs = 0;
let fcpMs = 0;
let domNodes = 0;
let jsHeapMB = 0;
let workspaceSwitchMs = null;
let projectShellMs = 0;
let projectWorkspaceSwitchMs = null;

try {
  await ensureAppServer();

  browser = await chromium.launch();
  const ctx = await browser.newContext({
    viewport: { width: 1400, height: 1000 },
    deviceScaleFactor: 1,
  });
  await ctx.addInitScript(`
    try {
      localStorage.setItem("awidat:welcome:shown", new Date().toISOString());
    } catch {}
  `);
  const page = await ctx.newPage();

  const warmupPage = await ctx.newPage();
  await warmupPage.goto(BASE_URL, { waitUntil: "networkidle" });
  await warmupPage.close();

  const warmupProjectPage = await ctx.newPage();
  await warmupProjectPage.goto(new URL("/tests/ui-harness.html?project=1", BASE_URL).toString(), {
    waitUntil: "networkidle",
  });
  await warmupProjectPage.close();

  console.log("→ measuring app load …");

  const t0 = Date.now();
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  loadMs = Date.now() - t0;
  console.log(`  load (network idle): ${loadMs} ms (budget ${BUDGETS.loadMs})`);

  const client = await page.context().newCDPSession(page);
  await client.send("Performance.enable");

  const metrics = await client.send("Performance.getMetrics");
  const heapBytes = metrics.metrics.find((m) => m.name === "JSHeapUsedSize")?.value ?? 0;
  jsHeapMB = heapBytes / (1024 * 1024);
  console.log(`  JS heap: ${jsHeapMB.toFixed(1)} MB (budget ${BUDGETS.jsHeapMB})`);

  const fcpEntries = await page.evaluate(() => {
    return performance
      .getEntriesByType("paint")
      .filter((e) => e.name === "first-contentful-paint")
      .map((e) => e.startTime);
  });
  fcpMs = fcpEntries[0] ?? 0;
  console.log(`  FCP: ${fcpMs.toFixed(0)} ms (budget ${BUDGETS.fcpMs})`);

  domNodes = await page.evaluate(() => document.querySelectorAll("*").length);
  console.log(`  DOM nodes: ${domNodes} (budget ${BUDGETS.domNodes})`);

  console.log("→ measuring workspace switching …");
  const workspaceTabs = page.locator('[role="tablist"][aria-label="Workspace"] button[role="tab"]');
  const tabCount = await workspaceTabs.count();
  if (tabCount === 0) {
    console.log("  workspace switch: skipped (workspace tablist not present)");
  } else {
    workspaceSwitchMs = 0;
    for (let index = 0; index < tabCount; index += 1) {
      const tab = workspaceTabs.nth(index);
      const selected = await tab.getAttribute("aria-selected");
      if (selected === "true") continue;
      const start = Date.now();
      await tab.click();
      await page.evaluate(
      () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
    );
    const elapsed = Date.now() - start;
    console.log(`    ${((await tab.textContent()) ?? "").trim()}: ${elapsed.toFixed(1)} ms`);
    workspaceSwitchMs = Math.max(workspaceSwitchMs, elapsed);
  }
    console.log(
      `  slowest workspace switch: ${workspaceSwitchMs.toFixed(1)} ms (budget ${BUDGETS.workspaceSwitchMs})`,
    );
  }

  await page.close();

  console.log("→ measuring loaded-project shell under slow background IPC …");
  const projectPage = await ctx.newPage();
  const projectUrl = new URL("/tests/ui-harness.html", BASE_URL);
  projectUrl.searchParams.set("project", "1");
  projectUrl.searchParams.set("slowNonCritical", "1");
  const projectStart = Date.now();
  await projectPage.goto(projectUrl.toString(), { waitUntil: "domcontentloaded" });
  const projectButtonNames = ["Deliver", "Schedule", "Skills"];
  await projectPage.getByRole("button", { name: "Stage" }).waitFor({ state: "visible" });
  projectShellMs = Date.now() - projectStart;
  console.log(`  project shell visible: ${projectShellMs} ms (budget ${BUDGETS.projectShellMs})`);

  await projectPage.waitForFunction(() => {
    const calls = window.__awidatIpcCalls ?? [];
    return ["current_project_root", "read_timeline", "list_source_media", "list_proxies"].every(
      (command) => calls.some((call) => call.command === command),
    );
  });
  const calls = await projectPage.evaluate(() => window.__awidatIpcCalls ?? null);
  if (!Array.isArray(calls)) {
    throw new Error("loaded-project harness did not expose __awidatIpcCalls");
  }
  for (const command of ["current_project_root", "read_timeline", "list_source_media", "list_proxies"]) {
    if (!calls.some((call) => call.command === command)) {
      throw new Error(`loaded-project critical path never invoked ${command}`);
    }
  }

  projectWorkspaceSwitchMs = 0;
  for (const name of projectButtonNames) {
    const tab = projectPage
      .locator("button")
      .filter({ hasText: new RegExp(`${name}$`) })
      .first();
    const start = Date.now();
    await tab.click();
    await projectPage.evaluate(
      () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
    );
    const elapsed = Date.now() - start;
    console.log(`    ${name}: ${elapsed.toFixed(1)} ms`);
    projectWorkspaceSwitchMs = Math.max(projectWorkspaceSwitchMs, elapsed);
  }
  console.log(
    `  project workspace switch: ${projectWorkspaceSwitchMs.toFixed(1)} ms (budget ${BUDGETS.projectWorkspaceSwitchMs})`,
  );
  await projectPage.close();
} finally {
  if (browser) await browser.close();
  await stopAppServer();
}

const failures = [];
if (loadMs > BUDGETS.loadMs) failures.push(`loadMs ${loadMs} > ${BUDGETS.loadMs}`);
if (fcpMs > BUDGETS.fcpMs) failures.push(`fcpMs ${fcpMs.toFixed(0)} > ${BUDGETS.fcpMs}`);
if (domNodes > BUDGETS.domNodes) failures.push(`domNodes ${domNodes} > ${BUDGETS.domNodes}`);
if (jsHeapMB > BUDGETS.jsHeapMB) failures.push(`jsHeapMB ${jsHeapMB.toFixed(1)} > ${BUDGETS.jsHeapMB}`);
if (workspaceSwitchMs !== null && workspaceSwitchMs > BUDGETS.workspaceSwitchMs) {
  failures.push(`workspaceSwitchMs ${workspaceSwitchMs.toFixed(1)} > ${BUDGETS.workspaceSwitchMs}`);
}
if (projectShellMs > BUDGETS.projectShellMs) {
  failures.push(`projectShellMs ${projectShellMs} > ${BUDGETS.projectShellMs}`);
}
if (
  projectWorkspaceSwitchMs !== null &&
  projectWorkspaceSwitchMs > BUDGETS.projectWorkspaceSwitchMs
) {
  failures.push(
    `projectWorkspaceSwitchMs ${projectWorkspaceSwitchMs.toFixed(1)} > ${BUDGETS.projectWorkspaceSwitchMs}`,
  );
}

if (failures.length > 0) {
  console.error(`\n${failures.length} budget violation(s):`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log("\nAll budgets pass.");
