#!/usr/bin/env node
/**
 * Performance budget — measures load + paint + render on the new shell
 * and fails if budgets are exceeded.
 *
 * Budgets are conservative starting points; tighten as the app stabilizes.
 * Run with `pnpm dev` on :1420.
 */

import { chromium } from "playwright";

const URL = process.env.PERF_URL ?? "http://localhost:1420/";

const BUDGETS = {
  loadMs: 3000,
  fcpMs: 1500,
  domNodes: 2000,
  jsHeapMB: 80,
  minFps: 50,
};

const browser = await chromium.launch();
const ctx = await browser.newContext({
  viewport: { width: 1400, height: 1000 },
  deviceScaleFactor: 1,
});
const page = await ctx.newPage();

console.log("→ measuring app load …");

const t0 = Date.now();
await page.goto(URL, { waitUntil: "networkidle" });
const loadMs = Date.now() - t0;
console.log(`  load (network idle): ${loadMs} ms (budget ${BUDGETS.loadMs})`);

const client = await page.context().newCDPSession(page);
await client.send("Performance.enable");

const metrics = await client.send("Performance.getMetrics");
const heapBytes = metrics.metrics.find((m) => m.name === "JSHeapUsedSize")?.value ?? 0;
const jsHeapMB = heapBytes / (1024 * 1024);
console.log(`  JS heap: ${jsHeapMB.toFixed(1)} MB (budget ${BUDGETS.jsHeapMB})`);

const fcpEntries = await page.evaluate(() => {
  return performance
    .getEntriesByType("paint")
    .filter((e) => e.name === "first-contentful-paint")
    .map((e) => e.startTime);
});
const fcpMs = fcpEntries[0] ?? 0;
console.log(`  FCP: ${fcpMs.toFixed(0)} ms (budget ${BUDGETS.fcpMs})`);

const domNodes = await page.evaluate(() => document.querySelectorAll("*").length);
console.log(`  DOM nodes: ${domNodes} (budget ${BUDGETS.domNodes})`);

console.log("→ measuring interactivity …");
const fps = await page.evaluate(async () => {
  return new Promise((resolve) => {
    const samples = [];
    let last = performance.now();
    let count = 0;
    function tick(now) {
      samples.push(now - last);
      last = now;
      count++;
      if (count < 60) {
        requestAnimationFrame(tick);
      } else {
        const avgFrameMs = samples.reduce((a, b) => a + b, 0) / samples.length;
        resolve(1000 / avgFrameMs);
      }
    }
    requestAnimationFrame(tick);
  });
});
console.log(`  avg FPS: ${fps.toFixed(1)} (budget ${BUDGETS.minFps})`);

await browser.close();

const failures = [];
if (loadMs > BUDGETS.loadMs) failures.push(`loadMs ${loadMs} > ${BUDGETS.loadMs}`);
if (fcpMs > BUDGETS.fcpMs) failures.push(`fcpMs ${fcpMs.toFixed(0)} > ${BUDGETS.fcpMs}`);
if (domNodes > BUDGETS.domNodes) failures.push(`domNodes ${domNodes} > ${BUDGETS.domNodes}`);
if (jsHeapMB > BUDGETS.jsHeapMB) failures.push(`jsHeapMB ${jsHeapMB.toFixed(1)} > ${BUDGETS.jsHeapMB}`);
if (fps < BUDGETS.minFps) failures.push(`fps ${fps.toFixed(1)} < ${BUDGETS.minFps}`);

if (failures.length > 0) {
  console.error(`\n${failures.length} budget violation(s):`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log("\nAll budgets pass.");
