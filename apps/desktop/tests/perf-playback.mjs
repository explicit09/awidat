#!/usr/bin/env node
/** Playback baseline for the production-minified Chromium renderer harness. */

import { chromium } from "playwright";
import { mkdirSync, renameSync, writeFileSync } from "node:fs";
import os from "node:os";

const BASE_URL = process.env.PERF_URL;
const OUT_DIR = process.env.PERF_PLAYBACK_OUT_DIR ?? process.env.PERF_OUT_DIR ?? "tests/perf-results";
const LABEL = process.env.PERF_LABEL ?? "current";
const WARMUPS = Number(process.env.PERF_PLAYBACK_WARMUPS ?? 3);
const SAMPLES = Number(process.env.PERF_PLAYBACK_SAMPLES ?? 15);
const PLAY_SECONDS = Number(process.env.PERF_PLAYBACK_DURATION_S ?? 4);
const SWEEP_SECONDS = Number(process.env.PERF_PLAYBACK_SWEEP_DURATION_S ?? 17.6);
const VIEWPORT = { width: 1400, height: 1000, deviceScaleFactor: 1 };
const CONTROL_RAF_MS = 500;
const CHROMIUM_ARGS = [
  "--disable-background-timer-throttling",
  "--disable-renderer-backgrounding",
  "--disable-backgrounding-occluded-windows",
  "--disable-features=CalculateNativeWinOcclusion",
];
const TARGETS = {
  controlRafP95Ms: 25,
  rafP95Ms: 25,
  rafOver25Ratio: 0.05,
  maxLongTaskMs: 50,
  droppedFrames: 0,
  minClockAdvanceS: Math.max(0.5, PLAY_SECONDS - 0.75),
  maxClockAdvanceS: PLAY_SECONDS + 1.25,
  validSamples: SAMPLES,
  minimumTimedCutCrossings: PLAY_SECONDS >= 3.25 ? 3 : Math.max(1, Math.floor(PLAY_SECONDS)),
  fullSweepDurationS: 17.5,
  boundaryObservationLagS: 0.1,
  activeFrameGapMs: 125,
  sourceTimeToleranceS: 0.08,
};

if (!BASE_URL) throw new Error("PERF_URL is required for production playback benchmarking");
if (!Number.isInteger(WARMUPS) || WARMUPS < 0) throw new Error("PERF_PLAYBACK_WARMUPS must be a non-negative integer");
if (!Number.isInteger(SAMPLES) || SAMPLES < 1) throw new Error("PERF_PLAYBACK_SAMPLES must be a positive integer");
if (!(PLAY_SECONDS > 0) || !(SWEEP_SECONDS >= TARGETS.fullSweepDurationS)) throw new Error("invalid playback durations");

mkdirSync(OUT_DIR, { recursive: true });

function percentile(values, p) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.max(0, Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1))];
}

function median(values) {
  return percentile(values, 50);
}

function mad(values) {
  const center = median(values);
  return center === null ? null : median(values.map((value) => Math.abs(value - center)));
}

function round(value) {
  return typeof value === "number" && Number.isFinite(value) ? Math.round(value * 1000) / 1000 : value;
}

function summary(values) {
  const finite = values.filter((value) => Number.isFinite(value));
  return { median: round(median(finite)), p95: round(percentile(finite, 95)), mad: round(mad(finite)), count: finite.length };
}

function metricDelta(before, after, name, multiplier = 1) {
  const value = (after[name] ?? Number.NaN) - (before[name] ?? Number.NaN);
  return Number.isFinite(value) ? value * multiplier : null;
}

function writeAtomically(path, content) {
  const temporary = `${path}.${process.pid}.tmp`;
  writeFileSync(temporary, content);
  renameSync(temporary, path);
}

function reportPaths() {
  const stem = `${OUT_DIR}/desktop-playback-performance-${LABEL}`;
  return { jsonPath: `${stem}.json`, htmlPath: `${stem}.html` };
}

function htmlReport(report) {
  const rows = Object.entries(report.summaries)
    .map(([name, values]) => `<tr><td>${name}</td><td>${values.median ?? "n/a"}</td><td>${values.p95 ?? "n/a"}</td><td>${values.mad ?? "n/a"}</td></tr>`)
    .join("");
  return `<!doctype html><meta charset="utf-8"><title>Playback Performance</title><style>body{font:14px system-ui;margin:24px;color:#17212b}table{border-collapse:collapse;width:min(920px,100%)}td,th{border-bottom:1px solid #d8dee4;padding:8px;text-align:left}.pass{color:#087443}.fail{color:#b42318}small{color:#667085}</style><h1>Playback performance</h1><p class="${report.passed ? "pass" : "fail"}">${report.passed ? "PASS" : "FAIL"} · ${report.validSamples}/${SAMPLES} valid timed samples</p><table><thead><tr><th>Diagnostic metric</th><th>Median</th><th>P95</th><th>MAD</th></tr></thead><tbody>${rows}</tbody></table><p><small>Warm-media, production-minified, headful Chromium renderer with mocked Tauri IPC; not native WebView/Tauri latency.</small></p>`;
}

async function installInstrumentation(context) {
  await context.addInitScript(() => {
    const metrics = {
      rafIntervals: [],
      longTasks: [],
      media: null,
      lastRaf: null,
      collecting: false,
    };
    window.__montagePlaybackMetrics = metrics;
    const tick = (now) => {
      if (metrics.collecting && metrics.lastRaf !== null) metrics.rafIntervals.push(now - metrics.lastRaf);
      metrics.lastRaf = now;
      window.requestAnimationFrame(tick);
    };
    window.requestAnimationFrame(tick);
    try {
      new PerformanceObserver((list) => {
        if (!metrics.collecting) return;
        for (const entry of list.getEntries()) metrics.longTasks.push({ duration: entry.duration, startTime: entry.startTime });
      }).observe({ type: "longtask", buffered: true });
    } catch {}
    metrics.start = () => {
      metrics.rafIntervals = [];
      metrics.longTasks = [];
      metrics.lastRaf = null;
      metrics.collecting = true;
      metrics.media = {
        errors: 0,
        ended: 0,
        waiting: 0,
        activeWaiting: 0,
        stalled: 0,
        activeStalled: 0,
        rvfc: 0,
        activeRvfc: 0,
        rvfcBySlot: [0, 0],
        activeRvfcBySlot: [0, 0],
        activeFrames: [],
        rvfcSupported: true,
        slots: [],
        slotTransitions: [],
        urls: [],
        playingTransitions: [],
        timelineTimes: [],
        segmentVisits: [],
        qualityLast: [],
        decodedFrames: 0,
        droppedFrames: 0,
      };
      const attach = (video, index) => {
        if (video.__montagePlaybackAttached) return;
        video.__montagePlaybackAttached = true;
        video.addEventListener("error", () => { metrics.media.errors += 1; });
        video.addEventListener("ended", () => { metrics.media.ended += 1; });
        const recordLoadEvent = (kind) => {
          if (!metrics.collecting || !metrics.media) return;
          metrics.media[kind] += 1;
          if (getComputedStyle(video).pointerEvents === "auto") {
            metrics.media[`active${kind[0].toUpperCase()}${kind.slice(1)}`] += 1;
          }
        };
        video.addEventListener("waiting", () => recordLoadEvent("waiting"));
        video.addEventListener("stalled", () => recordLoadEvent("stalled"));
        if (!video.requestVideoFrameCallback) {
          metrics.media.rvfcSupported = false;
          return;
        }
        const onFrame = (_now, metadata) => {
          if (metrics.collecting) {
            metrics.media.rvfc += 1;
            metrics.media.rvfcBySlot[index] += 1;
            if (getComputedStyle(video).pointerEvents === "auto") {
              metrics.media.activeRvfc += 1;
              metrics.media.activeRvfcBySlot[index] += 1;
              const state = window.__montagePlayback?.state();
              const segmentIndex = state ? Math.min(17, Math.max(0, Math.floor(state.timelineTime))) : null;
              const segment = segmentIndex === null ? null : window.__montagePlayback?.fixture?.segments?.[segmentIndex];
              metrics.media.activeFrames.push({
                slot: index,
                atMs: _now,
                timelineTime: state?.timelineTime ?? null,
                segmentIndex,
                mediaTime: metadata.mediaTime,
                presentedFrames: metadata.presentedFrames,
                expectedDisplayTime: metadata.expectedDisplayTime,
                expectedSourceStartS: segment?.sourceStartS ?? null,
                expectedSourceEndS: segment?.sourceEndS ?? null,
              });
            }
            metrics.media.lastPresentedFrames = metadata.presentedFrames;
            metrics.media.lastMediaTime = metadata.mediaTime;
          }
          video.requestVideoFrameCallback(onFrame);
        };
        video.requestVideoFrameCallback(onFrame);
      };
      document.querySelectorAll("video").forEach(attach);
    };
    metrics.stop = () => { metrics.collecting = false; };
    metrics.observe = () => {
      const media = metrics.media;
      if (!media || !window.__montagePlayback) return;
      const state = window.__montagePlayback.state();
      const videos = Array.from(document.querySelectorAll("video"));
      const activeSlot = videos.findIndex((video) => getComputedStyle(video).pointerEvents === "auto");
      media.slots.push(activeSlot);
      if (media.slotTransitions.at(-1)?.slot !== activeSlot) {
        media.slotTransitions.push({ slot: activeSlot, timelineTime: state.timelineTime, atMs: performance.now() });
      }
      for (const video of videos) {
        if (video.currentSrc && !media.urls.includes(video.currentSrc)) media.urls.push(video.currentSrc);
      }
      const last = media.playingTransitions.at(-1);
      if (last === undefined || last !== state.isPlaying) media.playingTransitions.push(state.isPlaying);
      media.timelineTimes.push(state.timelineTime);
      const segmentIndex = Math.min(17, Math.max(0, Math.floor(state.timelineTime)));
      if (media.segmentVisits.at(-1)?.index !== segmentIndex) {
        media.segmentVisits.push({ index: segmentIndex, timelineTime: state.timelineTime, atMs: performance.now() });
      }
      videos.forEach((video, index) => {
        const quality = video.getVideoPlaybackQuality?.();
        if (!quality) return;
        const current = { total: quality.totalVideoFrames, dropped: quality.droppedVideoFrames };
        const previous = media.qualityLast[index] ?? current;
        media.decodedFrames += current.total >= previous.total ? current.total - previous.total : current.total;
        media.droppedFrames += current.dropped >= previous.dropped ? current.dropped - previous.dropped : current.dropped;
        media.qualityLast[index] = current;
      });
    };
  });
}

async function cdpMetrics(client) {
  const response = await client.send("Performance.getMetrics");
  return Object.fromEntries(response.metrics.map((metric) => [metric.name, metric.value]));
}

async function waitForPlaybackReady(page) {
  await page.waitForFunction(() => window.__montagePlayback && document.querySelectorAll("video").length >= 2);
  const consent = page.getByRole("button", { name: "I understand", exact: true });
  if (await consent.count()) await consent.click();
  await page.evaluate(() => window.__montagePlayback.seek(0));
  await page.waitForFunction(() => {
    const state = window.__montagePlayback.state();
    return state.timelineTime <= 0.05 &&
      state.timelineDuration === 18 &&
      Array.from(document.querySelectorAll("video")).every(
        (video) => video.currentSrc && video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA,
      );
  });
  await page.waitForTimeout(300);
}

async function measureControlRaf(page) {
  const intervals = await page.evaluate((durationMs) => new Promise((resolve) => {
    const samples = [];
    const finishAt = performance.now() + durationMs;
    let previous = null;
    const tick = (now) => {
      if (previous !== null) samples.push(now - previous);
      previous = now;
      if (now >= finishAt) resolve(samples);
      else window.requestAnimationFrame(tick);
    };
    window.requestAnimationFrame(tick);
  }), CONTROL_RAF_MS);
  return {
    p50Ms: percentile(intervals, 50),
    p95Ms: percentile(intervals, 95),
    maxMs: intervals.length ? Math.max(...intervals) : null,
    count: intervals.length,
  };
}

async function clickTransport(page, label) {
  await page.getByRole("button", { name: label, exact: true }).click();
}

function validPlaybackFixture(fixture) {
  const segments = fixture?.segments;
  if (!Array.isArray(segments) || fixture.durationS !== 18 || segments.length !== 18) return false;
  return segments.every((segment, index) =>
    segment.index === index &&
    Math.abs(segment.startS - index) < 0.000_001 &&
    Math.abs(segment.endS - (index + 1)) < 0.000_001 &&
    segment.sourceStartS >= 0 &&
    segment.sourceEndS <= 1.75 &&
    typeof segment.playablePath === "string",
  ) && new Set(segments.map((segment) => segment.playablePath)).size === 2;
}

async function runPlayback(page, client, seconds, requireFullSweep) {
  await waitForPlaybackReady(page);
  const controlRaf = await measureControlRaf(page);
  await page.evaluate(() => window.__montagePlaybackMetrics.start());
  await page.evaluate(() => window.__montagePlaybackMetrics.observe());
  const beforeCdp = await cdpMetrics(client);
  const before = await page.evaluate(() => ({
    renderCount: window.__montagePerfAppRootRenderCount ?? null,
    state: window.__montagePlayback.state(),
  }));
  await clickTransport(page, "▶");
  await page.waitForFunction(() => window.__montagePlayback.state().isPlaying);
  const startedAt = await page.evaluate(() => performance.now());
  await page.evaluate(async (durationMs) => {
    await new Promise((resolve) => {
      const finishAt = performance.now() + durationMs;
      const observe = () => {
        window.__montagePlaybackMetrics.observe();
        if (performance.now() >= finishAt) resolve();
        else window.requestAnimationFrame(observe);
      };
      window.requestAnimationFrame(observe);
    });
  }, seconds * 1000);
  const endedAt = await page.evaluate(() => performance.now());
  await clickTransport(page, "❚❚");
  await page.waitForFunction(() => !window.__montagePlayback.state().isPlaying);
  await page.evaluate(() => window.__montagePlaybackMetrics.observe());
  await page.evaluate(() => window.__montagePlaybackMetrics.stop());
  const afterCdp = await cdpMetrics(client);
  const after = await page.evaluate(() => ({
    renderCount: window.__montagePerfAppRootRenderCount ?? null,
    state: window.__montagePlayback.state(),
    metrics: window.__montagePlaybackMetrics,
    fixture: window.__montagePlayback.fixture,
  }));
  const raf = after.metrics.rafIntervals;
  const longTasks = after.metrics.longTasks;
  const clockAdvanceS = after.state.timelineTime - before.state.timelineTime;
  const cutCrossings = Math.floor(after.state.timelineTime) - Math.floor(before.state.timelineTime);
  const resource = {
    appRootRenderAttempts: before.renderCount === null || after.renderCount === null ? null : after.renderCount - before.renderCount,
    taskDurationMs: metricDelta(beforeCdp, afterCdp, "TaskDuration", 1000),
    scriptDurationMs: metricDelta(beforeCdp, afterCdp, "ScriptDuration", 1000),
    layoutDurationMs: metricDelta(beforeCdp, afterCdp, "LayoutDuration", 1000),
    recalcStyleDurationMs: metricDelta(beforeCdp, afterCdp, "RecalcStyleDuration", 1000),
    jsHeapBefore: beforeCdp.JSHeapUsedSize ?? null,
    jsHeapAfter: afterCdp.JSHeapUsedSize ?? null,
    jsHeapDelta: metricDelta(beforeCdp, afterCdp, "JSHeapUsedSize"),
  };
  const finiteMetrics = Object.values(resource).every((value) => Number.isFinite(value));
  const media = after.metrics.media;
  const quality = {
    controlRafP95Ms: controlRaf.p95Ms,
    rafP50Ms: percentile(raf, 50),
    rafP95Ms: percentile(raf, 95),
    rafMaxMs: raf.length ? Math.max(...raf) : null,
    rafOver25Ratio: raf.length ? raf.filter((value) => value > 25).length / raf.length : null,
    maxLongTaskMs: longTasks.length ? Math.max(...longTasks.map((task) => task.duration)) : 0,
    decodedFramesDelta: media.decodedFrames,
    droppedFramesDelta: media.droppedFrames,
    activeVideoFrameCallbacks: media.activeRvfc,
  };
  const observedSlots = [...new Set(media.slots.filter((slot) => slot >= 0))];
  const observedUrls = [...new Set(media.urls)];
  const slotTransitions = media.slotTransitions.filter(({ slot }) => slot >= 0);
  const activeSlotChanges = Math.max(0, slotTransitions.length - 1);
  const orderedSegments = media.segmentVisits.map(({ index }) => index);
  const boundaryLags = media.segmentVisits
    .filter(({ index }) => index > 0)
    .map(({ index, timelineTime }) => timelineTime - index);
  const maxBoundaryObservationLagS = boundaryLags.length ? Math.max(...boundaryLags) : null;
  const expectedCuts = requireFullSweep ? 17 : TARGETS.minimumTimedCutCrossings;
  const expectedSegments = Array.from(
    { length: requireFullSweep ? 18 : Math.min(18, cutCrossings + 1) },
    (_, index) => index,
  );
  const finalRequiredFrameSegment = requireFullSweep
    ? 17
    : Math.min(17, Math.max(0, Math.floor(after.state.timelineTime - TARGETS.boundaryObservationLagS)));
  const requiredFrameSegments = Array.from({ length: finalRequiredFrameSegment + 1 }, (_, index) => index);
  const activeFrameSegments = new Set(media.activeFrames.map(({ segmentIndex }) => segmentIndex));
  const activeFrameGaps = media.activeFrames.slice(1).map((frame, index) => frame.atMs - media.activeFrames[index].atMs);
  const maxActiveFrameGapMs = activeFrameGaps.length ? Math.max(...activeFrameGaps) : null;
  const activeFrameSourceTimesValid = media.activeFrames.every((frame) =>
    Number.isFinite(frame.mediaTime) &&
    Number.isFinite(frame.expectedSourceStartS) &&
    Number.isFinite(frame.expectedSourceEndS) &&
    frame.mediaTime >= frame.expectedSourceStartS - TARGETS.sourceTimeToleranceS &&
    frame.mediaTime <= frame.expectedSourceEndS + TARGETS.sourceTimeToleranceS
  );
  const qualityMetricsFinite = Object.values(quality).every((value) => Number.isFinite(value));
  const correctness = {
    fixture: validPlaybackFixture(after.fixture),
    controlRaf: controlRaf.count >= 20 && controlRaf.p95Ms <= TARGETS.controlRafP95Ms,
    decoded: quality.decodedFramesDelta > 0 && media.activeRvfc > 0 && media.activeRvfcBySlot.every((count) => count > 0),
    clockAdvanced: clockAdvanceS >= (requireFullSweep ? TARGETS.fullSweepDurationS : TARGETS.minClockAdvanceS) && clockAdvanceS <= (requireFullSweep ? 19.5 : TARGETS.maxClockAdvanceS),
    cutCrossings: cutCrossings >= expectedCuts,
    playPause: media.playingTransitions[0] === false && media.playingTransitions.includes(true) && media.playingTransitions.at(-1) === false,
    noMediaError: after.state.mediaError === null && media.errors === 0,
    noEndedOrEarlyGap: media.ended === 0,
    noEarlyPause: media.playingTransitions.slice(1, -1).every(Boolean),
    rvfc: media.rvfcSupported && media.activeRvfc > 0,
    slotAndUrlHandoff: observedSlots.length === 2 && observedUrls.length >= 2,
    monotonicClock: media.timelineTimes.every((time, index, values) => index === 0 || time >= values[index - 1] - 0.05),
    derivedSegments: expectedSegments.every((segment, index) => orderedSegments[index] === segment),
    handoffCount: !requireFullSweep || activeSlotChanges === 17,
    boundaryTiming: maxBoundaryObservationLagS !== null && maxBoundaryObservationLagS <= TARGETS.boundaryObservationLagS,
    activeFramesAcrossSegments: requiredFrameSegments.every((segment) => activeFrameSegments.has(segment)),
    activeFrameSourceTimes: activeFrameSourceTimesValid,
    noActiveLoadStall: media.activeWaiting === 0 && media.activeStalled === 0,
    activeFrameCadence: maxActiveFrameGapMs !== null && maxActiveFrameGapMs <= TARGETS.activeFrameGapMs,
    finiteMetrics: finiteMetrics && qualityMetricsFinite,
  };
  return {
    fixture: after.fixture,
    elapsedMs: endedAt - startedAt,
    timeline: { start: before.state.timelineTime, end: after.state.timelineTime, duration: after.state.timelineDuration, advance: clockAdvanceS, cutCrossings },
    controlRaf,
    isPlayingTransitions: media.playingTransitions,
    media: {
      ...media,
      observedSlots,
      observedUrls,
      activeSlotChanges,
      orderedSegments,
      requiredFrameSegments,
      activeFrameSegments: [...activeFrameSegments].sort((a, b) => a - b),
      maxBoundaryObservationLagS,
      maxActiveFrameGapMs,
      error: after.state.mediaError,
    },
    resource,
    quality,
    correctness,
    valid: Object.values(correctness).every(Boolean),
  };
}

async function runFreshSample(browser, index, warmup) {
  const context = await browser.newContext(VIEWPORT);
  await installInstrumentation(context);
  const page = await context.newPage();
  await page.bringToFront();
  const client = await context.newCDPSession(page);
  await client.send("Performance.enable");
  await client.send("Emulation.setFocusEmulationEnabled", { enabled: true });
  try {
    await page.goto(new URL("/tests/ui-harness.html?project=1&scenario=playback", BASE_URL).toString(), { waitUntil: "domcontentloaded" });
    const result = await runPlayback(page, client, PLAY_SECONDS, false);
    const userAgent = await page.evaluate(() => navigator.userAgent);
    return { index, warmup, userAgent, ...result };
  } catch (error) {
    return { index, warmup, valid: false, error: String(error), correctness: { execution: false } };
  } finally {
    await context.close();
  }
}

async function runFullSweep(browser) {
  const context = await browser.newContext(VIEWPORT);
  await installInstrumentation(context);
  const page = await context.newPage();
  await page.bringToFront();
  const client = await context.newCDPSession(page);
  await client.send("Performance.enable");
  await client.send("Emulation.setFocusEmulationEnabled", { enabled: true });
  try {
    await page.goto(new URL("/tests/ui-harness.html?project=1&scenario=playback", BASE_URL).toString(), { waitUntil: "domcontentloaded" });
    return await runPlayback(page, client, SWEEP_SECONDS, true);
  } catch (error) {
    return { valid: false, error: String(error), correctness: { execution: false } };
  } finally {
    await context.close();
  }
}

const browser = await chromium.launch({ headless: false, args: CHROMIUM_ARGS });
try {
  const warmups = [];
  for (let index = 1; index <= WARMUPS; index += 1) warmups.push(await runFreshSample(browser, index, true));
  const samples = [];
  for (let index = 1; index <= SAMPLES; index += 1) samples.push(await runFreshSample(browser, index, false));
  const sweep = await runFullSweep(browser);
  const validSamples = samples.filter((sample) => sample.valid);
  const summaries = {
    appRootRenderAttempts: summary(validSamples.map((sample) => sample.resource.appRootRenderAttempts)),
    taskDurationMs: summary(validSamples.map((sample) => sample.resource.taskDurationMs)),
    scriptDurationMs: summary(validSamples.map((sample) => sample.resource.scriptDurationMs)),
    layoutDurationMs: summary(validSamples.map((sample) => sample.resource.layoutDurationMs)),
    recalcStyleDurationMs: summary(validSamples.map((sample) => sample.resource.recalcStyleDurationMs)),
    jsHeapDelta: summary(validSamples.map((sample) => sample.resource.jsHeapDelta)),
    controlRafP95Ms: summary(validSamples.map((sample) => sample.quality.controlRafP95Ms)),
    rafP95Ms: summary(validSamples.map((sample) => sample.quality.rafP95Ms)),
    rafOver25Ratio: summary(validSamples.map((sample) => sample.quality.rafOver25Ratio)),
    activeVideoFrameCallbacks: summary(validSamples.map((sample) => sample.quality.activeVideoFrameCallbacks)),
    maxActiveFrameGapMs: summary(validSamples.map((sample) => sample.media.maxActiveFrameGapMs)),
    decodedFramesDelta: summary(validSamples.map((sample) => sample.quality.decodedFramesDelta)),
    droppedFramesDelta: summary(validSamples.map((sample) => sample.quality.droppedFramesDelta)),
    maxLongTaskMs: summary(validSamples.map((sample) => sample.quality.maxLongTaskMs)),
  };
  const qualityFailures = samples.flatMap((sample) => {
    if (!sample.valid) return [`sample ${sample.index} correctness`];
    const failures = [];
    if (sample.quality.rafP95Ms > TARGETS.rafP95Ms) failures.push(`sample ${sample.index} rAF p95`);
    if (sample.quality.rafOver25Ratio > TARGETS.rafOver25Ratio) failures.push(`sample ${sample.index} rAF >25ms ratio`);
    if (sample.quality.maxLongTaskMs > TARGETS.maxLongTaskMs) failures.push(`sample ${sample.index} long task`);
    if (sample.quality.droppedFramesDelta !== TARGETS.droppedFrames) failures.push(`sample ${sample.index} dropped frames`);
    return failures;
  });
  const invalidWarmups = warmups.filter((warmup) => !warmup.valid);
  if (invalidWarmups.length > 0) qualityFailures.push(`${invalidWarmups.length}/${WARMUPS} invalid warmups`);
  if (validSamples.length !== TARGETS.validSamples) qualityFailures.push(`${validSamples.length}/${TARGETS.validSamples} valid samples`);
  if (!sweep.valid) qualityFailures.push("full correctness sweep");
  const report = {
    label: LABEL,
    generatedAt: new Date().toISOString(),
    scope: "warm-media production-minified headful Chromium renderer with mocked Tauri IPC; not native WebView/Tauri latency",
    baseUrl: BASE_URL,
    viewport: VIEWPORT,
    provenance: {
      browser: {
        engine: "Chromium",
        version: browser.version(),
        userAgent: warmups[0]?.userAgent ?? samples[0]?.userAgent ?? null,
        headless: false,
        launchArgs: CHROMIUM_ARGS,
      },
      environment: {
        node: process.version,
        platform: process.platform,
        release: os.release(),
        arch: process.arch,
      },
      cache: "fresh browser contexts; both alternating media URLs decoded before each measured playback window",
      buildEvidence: {
        path: process.env.PERF_BUILD_EVIDENCE_PATH ?? null,
        sha256: process.env.PERF_BUILD_EVIDENCE_SHA256 ?? null,
      },
      command: process.env.PERF_RUN_COMMAND ?? "node tests/perf-playback.mjs",
    },
    targets: TARGETS,
    warmups,
    samples,
    fullCorrectnessSweep: sweep,
    validSamples: validSamples.length,
    summaries,
    qualityFailures,
    passed: qualityFailures.length === 0,
  };
  const paths = reportPaths();
  writeAtomically(paths.jsonPath, `${JSON.stringify(report, null, 2)}\n`);
  writeAtomically(paths.htmlPath, htmlReport(report));
  console.log(JSON.stringify({ validSamples: report.validSamples, qualityFailures, summaries }, null, 2));
  console.log(`Wrote ${paths.jsonPath}`);
  console.log(`Wrote ${paths.htmlPath}`);
  if (!report.passed) process.exitCode = 1;
} finally {
  await browser.close();
}
