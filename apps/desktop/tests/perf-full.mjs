#!/usr/bin/env node
/**
 * Full UX performance benchmark for the desktop renderer path.
 *
 * This uses the Tauri mock harness so it can run headlessly in CI/dev
 * while still exercising the real React desktop shell, project hydration,
 * timeline preview, media URL IPC, and stage/menu switching.
 */

import { chromium } from "playwright";
import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, renameSync, writeFileSync } from "node:fs";
import os from "node:os";
import { setTimeout as delay } from "node:timers/promises";

const BASE_URL = process.env.PERF_URL ?? "http://localhost:1420/";
const OUT_DIR = process.env.PERF_OUT_DIR ?? "tests/perf-results";
const RUN_LABEL = process.env.PERF_LABEL ?? "current";
const SWITCH_SAMPLES = Number(process.env.PERF_SWITCH_SAMPLES ?? 8);
const SERVER_MODE = process.env.PERF_SERVER_MODE ?? (process.env.PERF_URL ? "external" : "vite-dev");
const VIEWPORT = { width: 1400, height: 1000, deviceScaleFactor: 1 };
const WORKSPACE_ROOT = new URL("../../..", import.meta.url);
const REQUIRED_TIMELINE_IPC = [
  "current_project_root",
  "read_timeline",
  "list_source_media",
  "list_proxies",
];
const MEDIA_URL_COMMAND = "media_url_for_path";
const PRESENTATION_FRAME_COUNT = 2;

const TARGETS = {
  warmStartupUsableShellMs: 2_500,
  coldStartupUsableShellMs: 4_000,
  openProjectToFirstPreviewFrameMs: 1_500,
  openProjectToInteractiveTimelineMs: 2_000,
  menuSwitchP95Ms: 120,
  maxAvoidableLongTaskMs: 50,
};

let appServer = null;
let appServerCommand = null;

mkdirSync(OUT_DIR, { recursive: true });

function commandOutput(command, args) {
  try {
    return execFileSync(command, args, { cwd: WORKSPACE_ROOT, encoding: "utf8" }).trim();
  } catch {
    return null;
  }
}

function provenance(browserVersion, userAgent) {
  return {
    server: {
      mode: SERVER_MODE,
      managedByBenchmark: appServer !== null,
      external: appServer === null,
      baseUrl: BASE_URL,
    },
    git: {
      head: commandOutput("git", ["rev-parse", "HEAD"]),
      branch: commandOutput("git", ["branch", "--show-current"]),
      dirty: commandOutput("git", ["status", "--porcelain"]),
    },
    environment: {
      node: process.version,
      platform: process.platform,
      release: os.release(),
      arch: process.arch,
      browser: { engine: "Chromium", version: browserVersion, userAgent },
      viewport: VIEWPORT,
      cache: "fresh Chromium context; one base-page warmup navigation before measurements",
      warmup: "base page loaded with networkidle before measured pages",
    },
    buildEvidence: {
      path: process.env.PERF_BUILD_EVIDENCE_PATH ?? null,
      sha256: process.env.PERF_BUILD_EVIDENCE_SHA256 ?? null,
    },
  };
}

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
  appServerCommand = "pnpm --dir apps/desktop exec vite --host 127.0.0.1";
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
      localStorage.setItem("montage:welcome:consent", new Date().toISOString());
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

async function waitForPresentationFrames(page, count = PRESENTATION_FRAME_COUNT) {
  const timestamps = await page.evaluate(async (frameCount) => {
    const frames = [];
    for (let index = 0; index < frameCount; index += 1) {
      const timestamp = await new Promise((resolve) => requestAnimationFrame(resolve));
      if (!Number.isFinite(timestamp)) throw new Error("presentation frame did not provide a finite timestamp");
      frames.push(timestamp);
    }
    return frames;
  }, count);
  if (timestamps.length !== count || timestamps.some((timestamp) => !Number.isFinite(timestamp))) {
    throw new Error(`expected ${count} finite presentation frames`);
  }
  return timestamps;
}

async function measureLoadedProject(page) {
  const navStart = Date.now();
  await page.goto(projectHarnessUrl(), { waitUntil: "domcontentloaded" });
  await page.getByRole("button", { name: "Chat", exact: true }).waitFor({ state: "visible" });
  const shellInteractiveMs = Date.now() - navStart;

  await page.waitForFunction((requiredCommands) => {
    const calls = window.__montageIpcCalls ?? [];
    return requiredCommands.every((command) =>
      calls.some(
        (call) =>
          call.command === command &&
          Number.isFinite(call.atMs) &&
          Number.isFinite(call.resolvedAtMs) &&
          call.resolvedAtMs >= call.atMs,
      ),
    );
  }, REQUIRED_TIMELINE_IPC);
  await page.waitForFunction(() => {
    if (document.querySelector(".timeline-empty")) return false;
    const canvas = document.querySelector("canvas.timeline-canvas");
    if (!(canvas instanceof HTMLCanvasElement)) return false;
    const { width: cssWidth, height: cssHeight } = canvas.getBoundingClientRect();
    if (![cssWidth, cssHeight, canvas.width, canvas.height].every((dimension) => Number.isFinite(dimension) && dimension > 0)) {
      return false;
    }
    try {
      return (canvas.getContext("2d")?.getImageData(0, 0, 1, 1).data[3] ?? 0) > 0;
    } catch {
      return false;
    }
  });
  const timelinePresentationFrames = await waitForPresentationFrames(page);
  if (await page.evaluate(() => document.querySelector(".timeline-empty") !== null)) {
    throw new Error("timeline became empty before interactive readiness");
  }
  const timelineInteractiveMs = Date.now() - navStart;

  await page.waitForFunction((command) => {
    const calls = window.__montageIpcCalls ?? [];
    return calls.some(
      (call) =>
        call.command === command &&
        Number.isFinite(call.atMs) &&
        Number.isFinite(call.resolvedAtMs) &&
        call.resolvedAtMs >= call.atMs,
    );
  }, MEDIA_URL_COMMAND);
  await page.waitForFunction(() =>
    Array.from(document.querySelectorAll("video")).some(
      (video) => video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA,
    ),
  );
  const previewPresentationFrames = await waitForPresentationFrames(page);
  const firstPreviewFrameMs = Date.now() - navStart;

  const { perf, ipcCalls, readiness } = await page.evaluate(({ requiredCommands, mediaUrlCommand }) => {
    const calls = window.__montageIpcCalls ?? [];
    const canvas = document.querySelector("canvas.timeline-canvas");
    const rect = canvas?.getBoundingClientRect();
    const emptyStatePresent = document.querySelector(".timeline-empty") !== null;
    const timelineCanvas = {
      selector: "canvas.timeline-canvas",
      present: canvas instanceof HTMLCanvasElement,
      painted: false,
      cssWidth: rect?.width ?? 0,
      cssHeight: rect?.height ?? 0,
      backingWidth: canvas?.width ?? 0,
      backingHeight: canvas?.height ?? 0,
      emptyStatePresent,
    };
    if (canvas instanceof HTMLCanvasElement) {
      try {
        timelineCanvas.painted = (canvas.getContext("2d")?.getImageData(0, 0, 1, 1).data[3] ?? 0) > 0;
      } catch {}
    }
    const currentDataVideo = Array.from(document.querySelectorAll("video")).find(
      (video) => video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA,
    );
    return {
      perf: window.__montagePerf,
      ipcCalls: calls,
      readiness: {
        timeline: {
          requiredIpc: requiredCommands.map((command) => {
            const call = calls.find(
              (candidate) =>
                candidate.command === command &&
                Number.isFinite(candidate.atMs) &&
                Number.isFinite(candidate.resolvedAtMs) &&
                candidate.resolvedAtMs >= candidate.atMs,
            );
            return {
              command,
              atMs: call?.atMs ?? null,
              resolvedAtMs: call?.resolvedAtMs ?? null,
            };
          }),
          canvas: timelineCanvas,
        },
        firstPreviewFrame: {
          mediaUrlIpc: (() => {
            const call = calls.find(
              (candidate) =>
                candidate.command === mediaUrlCommand &&
                Number.isFinite(candidate.atMs) &&
                Number.isFinite(candidate.resolvedAtMs) &&
                candidate.resolvedAtMs >= candidate.atMs,
            );
            return {
              command: mediaUrlCommand,
              atMs: call?.atMs ?? null,
              resolvedAtMs: call?.resolvedAtMs ?? null,
            };
          })(),
          currentData: currentDataVideo !== undefined,
          readyState: currentDataVideo?.readyState ?? null,
        },
      },
    };
  }, { requiredCommands: REQUIRED_TIMELINE_IPC, mediaUrlCommand: MEDIA_URL_COMMAND });
  readiness.timeline.presentationRafTimestamps = timelinePresentationFrames;
  readiness.firstPreviewFrame.presentationRafTimestamps = previewPresentationFrames;
  if (readiness.timeline.canvas.emptyStatePresent) {
    throw new Error("timeline became empty before readiness evidence was recorded");
  }
  const mediaUrlCall = ipcCalls.find(
    (call) => call.command === MEDIA_URL_COMMAND && Number.isFinite(call.resolvedAtMs),
  );
  const previewFrameMark = perf?.marks?.firstPreviewFrame ?? null;
  const previewEngineInitMs =
    mediaUrlCall && previewFrameMark !== null ? previewFrameMark - mediaUrlCall.resolvedAtMs : null;

  return {
    shellInteractiveMs,
    timelineInteractiveMs,
    firstPreviewFrameMs,
    previewEngineInitMs,
    perf,
    ipcCalls,
    readiness,
  };
}

async function measureSwitches(page) {
  const switches = [
    { name: "Deliver", shortcut: "2", heading: "Deliver" },
    { name: "Schedule", shortcut: "3", heading: "Schedule" },
    { name: "Skills", shortcut: "4", heading: "Skills" },
    { name: "Edit", shortcut: "1" },
  ];
  const samples = [];
  for (let iteration = 0; iteration < SWITCH_SAMPLES; iteration += 1) {
    for (const { name, shortcut, heading } of switches) {
      const result = await page.evaluate(async ({ shortcutKey, destinationHeading, presentationFrameCount }) => {
        const isVisible = (element) =>
          !!(element.offsetWidth || element.offsetHeight || element.getClientRects().length);
        const destinationReady = () => {
          const backToStage = Array.from(document.querySelectorAll("button")).some(
            (button) => button.textContent?.trim() === "← Stage" && isVisible(button),
          );
          const headingReady = Array.from(document.querySelectorAll("h1, h2")).some(
            (candidate) =>
              isVisible(candidate) &&
              (candidate.textContent?.trim() === destinationHeading ||
                candidate.textContent?.trim().startsWith(`${destinationHeading} `)),
          );
          return backToStage && headingReady;
        };
        const editReady = () =>
          ["Media", "Chat"].every((label) =>
            Array.from(document.querySelectorAll("button")).some(
              (button) => button.textContent?.trim() === label && isVisible(button),
            ),
          );
        const startedAt = performance.now();
        window.dispatchEvent(
          new KeyboardEvent("keydown", {
            key: shortcutKey,
            metaKey: navigator.platform.includes("Mac"),
            ctrlKey: !navigator.platform.includes("Mac"),
          }),
        );
        const ready = destinationHeading ? destinationReady : editReady;
        await new Promise((resolve, reject) => {
          const observer = new MutationObserver(() => {
            if (ready()) {
              cleanup();
              resolve();
            }
          });
          const timeout = window.setTimeout(() => {
            cleanup();
            reject(new Error(`workspace transition did not complete: ${destinationHeading ?? "Edit"}`));
          }, 1_000);
          const cleanup = () => {
            observer.disconnect();
            window.clearTimeout(timeout);
          };
          observer.observe(document.body, { childList: true, subtree: true, attributes: true });
          if (ready()) {
            cleanup();
            resolve();
          }
        });
        const presentationRafTimestamps = [];
        for (let frame = 0; frame < presentationFrameCount; frame += 1) {
          const timestamp = await new Promise((resolve) => requestAnimationFrame(resolve));
          if (!Number.isFinite(timestamp)) throw new Error("workspace presentation frame did not provide a finite timestamp");
          presentationRafTimestamps.push(timestamp);
        }
        return { durationMs: performance.now() - startedAt, presentationRafTimestamps };
      }, {
        shortcutKey: shortcut,
        destinationHeading: heading,
        presentationFrameCount: PRESENTATION_FRAME_COUNT,
      });
      samples.push({
        name,
        iteration,
        durationMs: result.durationMs,
        presentationRafTimestamps: result.presentationRafTimestamps,
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

function writeAtomically(path, content) {
  const temporaryPath = `${path}.${process.pid}.tmp`;
  writeFileSync(temporaryPath, content);
  renameSync(temporaryPath, path);
}

function writeReports(report) {
  const jsonPath = `${OUT_DIR}/desktop-ux-performance-${RUN_LABEL}.json`;
  const mdPath = `${OUT_DIR}/desktop-ux-performance-${RUN_LABEL}.md`;
  const htmlPath = `${OUT_DIR}/desktop-ux-performance-${RUN_LABEL}.html`;
  writeAtomically(jsonPath, JSON.stringify(report, null, 2));
  writeAtomically(mdPath, markdownReport(report));
  writeAtomically(htmlPath, htmlReport(report));
  return { jsonPath, mdPath, htmlPath };
}

function reportRows(report) {
  return [
    ["Warm startup to usable shell", report.metrics.warmStartupUsableShellMs, TARGETS.warmStartupUsableShellMs],
    ["Cold startup to usable shell", report.metrics.coldStartupUsableShellMs, TARGETS.coldStartupUsableShellMs],
    ["Open project to first preview frame", report.metrics.openProjectToFirstPreviewFrameMs, TARGETS.openProjectToFirstPreviewFrameMs],
    ["Open project to interactive timeline", report.metrics.openProjectToInteractiveTimelineMs, TARGETS.openProjectToInteractiveTimelineMs],
    ["Menu/tab switch p95", report.metrics.menuSwitchP95Ms, TARGETS.menuSwitchP95Ms],
    ["Max UI long task", report.metrics.maxLongTaskMs, TARGETS.maxAvoidableLongTaskMs],
  ];
}

function scopeDescription() {
  const renderer =
    SERVER_MODE === "production-minified"
      ? "Production tier measures the minified Chromium renderer"
      : SERVER_MODE === "vite-dev"
        ? "Development tier measures the Vite development Chromium renderer"
        : `External tier measures a Chromium renderer served in ${SERVER_MODE} mode`;
  return `${renderer} with mocked Tauri IPC; it is not native WebView/Tauri latency.`;
}

function markdownReport(report) {
  const rows = reportRows(report);
  return `# Desktop UX Performance Report

Label: ${report.label}
Generated: ${report.generatedAt}
Server: ${report.provenance.server.mode} (${report.provenance.server.managedByBenchmark ? "managed" : "external"})

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
- ${scopeDescription()}
- Cold native process start remains unsupported.

## Switch Samples

Menu switch p95 is computed from ${report.switches.samples.length} samples.

${report.switches.samples
  .map((sample) => `- ${sample.name} #${sample.iteration}: ${round(sample.durationMs)} ms`)
  .join("\n")}

## Long Tasks

Observed ${report.longTasks.length} long task(s) over 50 ms. Max: ${round(report.metrics.maxLongTaskMs) ?? 0} ms.

## Readiness Evidence

- Required timeline IPC responses: ${report.readiness.timeline.requiredIpc.map((call) => `${call.command} (${round(call.resolvedAtMs)} ms)`).join(", ")}
- Timeline canvas: ${round(report.readiness.timeline.canvas.cssWidth)} × ${round(report.readiness.timeline.canvas.cssHeight)} CSS px; ${report.readiness.timeline.canvas.backingWidth} × ${report.readiness.timeline.canvas.backingHeight} backing px; painted ${report.readiness.timeline.canvas.painted}; empty state present ${report.readiness.timeline.canvas.emptyStatePresent}.
- Timeline presentation frames: ${report.readiness.timeline.presentationRafTimestamps.length}.
- Preview media URL response: ${round(report.readiness.firstPreviewFrame.mediaUrlIpc.resolvedAtMs)} ms. Current data: ${report.readiness.firstPreviewFrame.currentData} (readyState ${report.readiness.firstPreviewFrame.readyState}); presentation frames: ${report.readiness.firstPreviewFrame.presentationRafTimestamps.length}.

## Commands

${report.commands.map((command) => `- \`${command}\``).join("\n")}
`;
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (character) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[character]);
}

function htmlReport(report) {
  const rows = reportRows(report)
    .map(([name, result, target]) => `<tr><td>${escapeHtml(name)}</td><td>${result === null ? "not measured" : `${round(result)} ms`}</td><td>${target} ms</td><td class="${status(result, target)}">${status(result, target)}</td></tr>`)
    .join("");
  const readiness = report.readiness;
  return `<!doctype html><meta charset="utf-8"><title>Desktop UX Performance</title><style>body{font:14px system-ui;margin:24px;color:#17212b}table{border-collapse:collapse;width:min(900px,100%)}td,th{border-bottom:1px solid #d8dee4;padding:8px;text-align:left}.pass{color:#087443}.fail{color:#b42318}.not_measured{color:#667085}small{color:#667085}</style><h1>Desktop UX Performance</h1><p>${escapeHtml(report.label)} · ${escapeHtml(report.provenance.server.mode)} (${report.provenance.server.managedByBenchmark ? "managed" : "external"})</p><table><thead><tr><th>Metric</th><th>Result</th><th>Target</th><th>Status</th></tr></thead><tbody>${rows}</tbody></table><h2>Readiness evidence</h2><ul><li>Required timeline IPC responses: ${escapeHtml(readiness.timeline.requiredIpc.map((call) => call.command).join(", "))}</li><li>Timeline canvas: ${round(readiness.timeline.canvas.cssWidth)} × ${round(readiness.timeline.canvas.cssHeight)} CSS px; ${readiness.timeline.canvas.backingWidth} × ${readiness.timeline.canvas.backingHeight} backing px; painted ${readiness.timeline.canvas.painted}; empty state present ${readiness.timeline.canvas.emptyStatePresent}.</li><li>Timeline presentation frames: ${readiness.timeline.presentationRafTimestamps.length}; preview media URL response: ${round(readiness.firstPreviewFrame.mediaUrlIpc.resolvedAtMs)} ms; preview current data: ${readiness.firstPreviewFrame.currentData}; preview presentation frames: ${readiness.firstPreviewFrame.presentationRafTimestamps.length}.</li></ul><p><small>${escapeHtml(scopeDescription())} Cold native process start remains unsupported.</small></p>`;
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
  const userAgent = await projectPage.evaluate(() => navigator.userAgent);
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
    readiness: project.readiness,
    marks: {
      firstVideoAttachedMs: round(project.perf?.marks?.firstVideoAttached ?? null),
      firstPreviewMetadataMs: round(project.perf?.marks?.firstPreviewMetadata ?? null),
      firstPreviewFrameMs: round(project.perf?.marks?.firstPreviewFrame ?? null),
    },
    ipcSummary: project.ipcCalls.map((call) => ({
      command: call.command,
      atMs: round(call.atMs),
      resolvedAtMs: round(call.resolvedAtMs),
    })),
    provenance: provenance(browser.version(), userAgent),
    commands: [
      process.env.PERF_RUN_COMMAND ?? "node tests/perf-full.mjs",
      ...(process.env.PERF_BUILD_COMMAND ? [process.env.PERF_BUILD_COMMAND] : []),
      ...(process.env.PERF_SERVER_COMMAND ? [process.env.PERF_SERVER_COMMAND] : []),
      ...(appServerCommand ? [appServerCommand] : []),
    ],
  };

  const paths = writeReports(report);
  console.log(JSON.stringify(report.metrics, null, 2));
  console.log(`\nWrote ${paths.jsonPath}`);
  console.log(`Wrote ${paths.mdPath}`);
  console.log(`Wrote ${paths.htmlPath}`);
  const failures = reportRows(report).filter(([, value, target]) => value !== null && value > target);
  if (failures.length > 0) {
    console.error(`\n${failures.length} performance target violation(s):`);
    for (const [name, value, target] of failures) {
      console.error(`  ${name} ${value} > ${target}`);
    }
    process.exitCode = 1;
  }
} finally {
  if (browser) await browser.close();
  await stopAppServer();
}
