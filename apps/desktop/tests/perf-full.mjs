#!/usr/bin/env node
/**
 * Full UX performance benchmark for the desktop renderer path.
 *
 * This uses the Tauri mock harness so it can run headlessly in CI/dev
 * while still exercising the real React desktop shell, project hydration,
 * timeline preview, media URL IPC, and stage/menu switching.
 */

import { chromium } from "playwright";
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";

const BASE_URL = process.env.PERF_URL ?? "http://localhost:1420/";
const OUT_DIR = process.env.PERF_OUT_DIR ?? "tests/perf-results";
const RUN_LABEL = process.env.PERF_LABEL ?? "current";
const SWITCH_SAMPLES = Number(process.env.PERF_SWITCH_SAMPLES ?? 8);

const TARGETS = {
  warmStartupUsableShellMs: 2_500,
  coldStartupUsableShellMs: 4_000,
  openProjectToFirstPreviewFrameMs: 1_500,
  openProjectToInteractiveTimelineMs: 2_000,
  menuSwitchP95Ms: 120,
  maxAvoidableLongTaskMs: 50,
};

let appServer = null;

mkdirSync(OUT_DIR, { recursive: true });

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

  const startedAt = Date.now();
  appServer = spawn("pnpm", ["--dir", "apps/desktop", "exec", "vite", "--host", "127.0.0.1"], {
    cwd: new URL("../../..", import.meta.url),
    env: { ...process.env, BROWSER: "none" },
    detached: process.platform !== "win32",
    stdio: ["ignore", "pipe", "pipe"],
  });

  appServer.stdout.on("data", (chunk) => process.stdout.write(chunk));
  appServer.stderr.on("data", (chunk) => process.stderr.write(chunk));

  for (let attempt = 0; attempt < 80; attempt += 1) {
    if (await canReachApp()) return Date.now() - startedAt;
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
    await Promise.race([stopped, delay(2_000)]);
  }
}

function percentile(values, p) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const index = Math.ceil((p / 100) * sorted.length) - 1;
  return sorted[Math.max(0, Math.min(sorted.length - 1, index))];
}

function round(value) {
  return typeof value === "number" && Number.isFinite(value)
    ? Math.round(value * 10) / 10
    : value;
}

function status(value, target, direction = "lte") {
  if (value === null || value === undefined) return "not_measured";
  return direction === "lte" ? (value <= target ? "pass" : "fail") : value >= target ? "pass" : "fail";
}

function projectHarnessUrl() {
  const url = new URL("/tests/ui-harness.html", BASE_URL);
  url.searchParams.set("project", "1");
  url.searchParams.set("slowNonCritical", "1");
  return url.toString();
}

async function installPageInstrumentation(context) {
  await context.addInitScript(`
    try {
      localStorage.setItem("montage:welcome:shown", new Date().toISOString());
    } catch {}
    window.__montagePerf = {
      longTasks: [],
      marks: {},
    };
    try {
      const observer = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          window.__montagePerf.longTasks.push({
            name: entry.name,
            startTime: entry.startTime,
            duration: entry.duration,
          });
        }
      });
      observer.observe({ type: "longtask", buffered: true });
    } catch {}
    const observePreviewVideo = () => {
      const markPreviewVideo = () => {
        if (window.__montagePerf.marks.firstVideoAttached) return;
        const video = document.querySelector("video");
        if (!video) return;
        window.__montagePerf.marks.firstVideoAttached = performance.now();
        video.addEventListener("loadedmetadata", () => {
          window.__montagePerf.marks.firstPreviewMetadata = performance.now();
        }, { once: true });
        video.addEventListener("loadeddata", () => {
          window.__montagePerf.marks.firstPreviewFrame = performance.now();
        }, { once: true });
        if (video.readyState >= HTMLMediaElement.HAVE_METADATA) {
          window.__montagePerf.marks.firstPreviewMetadata = performance.now();
        }
        if (video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA) {
          window.__montagePerf.marks.firstPreviewFrame = performance.now();
        }
      };
      markPreviewVideo();
      new MutationObserver(markPreviewVideo).observe(document.documentElement, {
        childList: true,
        subtree: true,
      });
    };
    if (document.documentElement) {
      observePreviewVideo();
    } else {
      document.addEventListener("DOMContentLoaded", observePreviewVideo, { once: true });
    }
  `);
}

async function measureWarmShell(page) {
  const startedAt = Date.now();
  await page.goto(BASE_URL, { waitUntil: "domcontentloaded" });
  await page
    .locator('[role="tablist"][aria-label="Workspace"] button[role="tab"]')
    .first()
    .waitFor({ state: "visible" });
  const shellInteractiveMs = Date.now() - startedAt;
  const fcpMs = await firstContentfulPaint(page);
  return { shellInteractiveMs, fcpMs };
}

async function firstContentfulPaint(page) {
  const entries = await page.evaluate(() =>
    performance
      .getEntriesByType("paint")
      .filter((entry) => entry.name === "first-contentful-paint")
      .map((entry) => entry.startTime),
  );
  return entries[0] ?? null;
}

async function measureLoadedProject(page) {
  const navStart = Date.now();
  await page.goto(projectHarnessUrl(), { waitUntil: "domcontentloaded" });
  await page.getByRole("button", { name: "Stage" }).waitFor({ state: "visible" });
  const shellInteractiveMs = Date.now() - navStart;

  await page.waitForFunction(() => {
    const calls = window.__montageIpcCalls ?? [];
    return ["current_project_root", "read_timeline", "list_source_media", "list_proxies"].every(
      (command) => calls.some((call) => call.command === command),
    );
  });
  const timelineInteractiveMs = Date.now() - navStart;

  await page.waitForFunction(() =>
    Array.from(document.querySelectorAll("video")).some(
      (video) => video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA,
    ),
  );
  const firstPreviewFrameMs = Date.now() - navStart;

  const perf = await page.evaluate(() => window.__montagePerf);
  const ipcCalls = await page.evaluate(() => window.__montageIpcCalls ?? []);
  const mediaUrlCall = ipcCalls.find((call) => call.command === "media_url_for_path");
  const previewFrameMark = perf?.marks?.firstPreviewFrame ?? null;
  const previewEngineInitMs =
    mediaUrlCall && previewFrameMark !== null ? previewFrameMark - mediaUrlCall.atMs : null;

  return {
    shellInteractiveMs,
    timelineInteractiveMs,
    firstPreviewFrameMs,
    previewEngineInitMs,
    perf,
    ipcCalls,
  };
}

async function measureSwitches(page) {
  const switches = [
    { id: "deliver", name: "Deliver" },
    { id: "schedule", name: "Schedule" },
    { id: "skills", name: "Skills" },
    { id: "edit", name: "Stage" },
  ];
  const samples = [];
  for (let iteration = 0; iteration < SWITCH_SAMPLES; iteration += 1) {
    for (const { id, name } of switches) {
      const durationMs = await page.evaluate(async (stageId) => {
        const button = document.querySelector(`[data-perf-stage-switch="${stageId}"]`);
        if (!button) throw new Error(`stage button not found: ${stageId}`);
        const startedAt = performance.now();
        button.click();
        for (let attempt = 0; attempt < 20; attempt += 1) {
          if (button.getAttribute("data-active") === "true") {
            return performance.now() - startedAt;
          }
          await Promise.resolve();
        }
        await new Promise((resolve) => setTimeout(resolve, 0));
        if (button.getAttribute("data-active") !== "true") {
          throw new Error(`stage button did not become active: ${stageId}`);
        }
        return performance.now() - startedAt;
      }, id);
      samples.push({
        name,
        iteration,
        durationMs,
      });
    }
  }
  return {
    samples,
    p95Ms: percentile(samples.map((sample) => sample.durationMs), 95),
    maxMs: Math.max(...samples.map((sample) => sample.durationMs)),
  };
}

async function collectLongTasks(page) {
  return page.evaluate(() => window.__montagePerf?.longTasks ?? []);
}

function writeReports(report) {
  const jsonPath = `${OUT_DIR}/desktop-ux-performance-${RUN_LABEL}.json`;
  const mdPath = `${OUT_DIR}/desktop-ux-performance-${RUN_LABEL}.md`;
  writeFileSync(jsonPath, JSON.stringify(report, null, 2));
  writeFileSync(mdPath, markdownReport(report));
  return { jsonPath, mdPath };
}

function markdownReport(report) {
  const rows = [
    ["Warm startup to usable shell", report.metrics.warmStartupUsableShellMs, TARGETS.warmStartupUsableShellMs],
    ["Cold startup to usable shell", report.metrics.coldStartupUsableShellMs, TARGETS.coldStartupUsableShellMs],
    ["Open project to first preview frame", report.metrics.openProjectToFirstPreviewFrameMs, TARGETS.openProjectToFirstPreviewFrameMs],
    ["Open project to interactive timeline", report.metrics.openProjectToInteractiveTimelineMs, TARGETS.openProjectToInteractiveTimelineMs],
    ["Menu/tab switch p95", report.metrics.menuSwitchP95Ms, TARGETS.menuSwitchP95Ms],
    ["Max UI long task", report.metrics.maxLongTaskMs, TARGETS.maxAvoidableLongTaskMs],
  ];
  return `# Desktop UX Performance Report

Label: ${report.label}
Generated: ${report.generatedAt}

## Metrics

| Metric | Result | Target | Status |
| --- | ---: | ---: | --- |
${rows
  .map(([name, result, target]) => {
    const rendered = result === null ? "not measured" : `${round(result)} ms`;
    return `| ${name} | ${rendered} | ${target} ms | ${status(result, target)} |`;
  })
  .join("\n")}

## Unsupported Native Metrics

- Native process start to renderer ready is not measured by this harness.
- This benchmark uses the desktop React renderer with Tauri IPC mocks, not a native WebView/WebDriver session.

## Switch Samples

Menu switch p95 is computed from ${report.switches.samples.length} samples.

${report.switches.samples
  .map((sample) => `- ${sample.name} #${sample.iteration}: ${round(sample.durationMs)} ms`)
  .join("\n")}

## Long Tasks

Observed ${report.longTasks.length} long task(s) over 50 ms. Max: ${round(report.metrics.maxLongTaskMs) ?? 0} ms.

## Commands

${report.commands.map((command) => `- \`${command}\``).join("\n")}
`;
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

try {
  const serverStartupMs = await ensureAppServer();
  browser = await chromium.launch();
  const context = await browser.newContext({
    viewport: { width: 1400, height: 1000 },
    deviceScaleFactor: 1,
  });
  await installPageInstrumentation(context);

  const warmup = await context.newPage();
  await warmup.goto(BASE_URL, { waitUntil: "networkidle" });
  await warmup.close();

  const warmShellPage = await context.newPage();
  const warmShell = await measureWarmShell(warmShellPage);
  await warmShellPage.close();

  const projectPage = await context.newPage();
  const project = await measureLoadedProject(projectPage);
  const switches = await measureSwitches(projectPage);
  const longTasks = await collectLongTasks(projectPage);
  await projectPage.close();

  const maxLongTaskMs = longTasks.length
    ? Math.max(...longTasks.map((task) => task.duration))
    : 0;

  const report = {
    label: RUN_LABEL,
    generatedAt: new Date().toISOString(),
    baseUrl: BASE_URL,
    targets: TARGETS,
    metrics: {
      processStartToRendererReadyMs: null,
      rendererReadyToShellInteractiveMs: round(warmShell.shellInteractiveMs),
      warmStartupUsableShellMs: round(warmShell.shellInteractiveMs),
      coldStartupUsableShellMs: serverStartupMs === undefined ? null : round(serverStartupMs + warmShell.shellInteractiveMs),
      openProjectToTimelineInteractiveMs: round(project.timelineInteractiveMs),
      openProjectToInteractiveTimelineMs: round(project.timelineInteractiveMs),
      openProjectToFirstPreviewFrameMs: round(project.firstPreviewFrameMs),
      previewEngineInitializationMs: round(project.previewEngineInitMs),
      menuSwitchP95Ms: round(switches.p95Ms),
      menuSwitchMaxMs: round(switches.maxMs),
      maxLongTaskMs: round(maxLongTaskMs),
    },
    switches,
    longTasks,
    marks: {
      firstVideoAttachedMs: round(project.perf?.marks?.firstVideoAttached ?? null),
      firstPreviewMetadataMs: round(project.perf?.marks?.firstPreviewMetadata ?? null),
      firstPreviewFrameMs: round(project.perf?.marks?.firstPreviewFrame ?? null),
    },
    ipcSummary: project.ipcCalls.map((call) => ({
      command: call.command,
      atMs: round(call.atMs),
    })),
    commands: [
      "npm run test:perf-full",
      "pnpm exec vite --host 127.0.0.1",
    ],
  };

  const paths = writeReports(report);
  console.log(JSON.stringify(report.metrics, null, 2));
  console.log(`\nWrote ${paths.jsonPath}`);
  console.log(`Wrote ${paths.mdPath}`);
} finally {
  if (browser) await browser.close();
  await stopAppServer();
}
