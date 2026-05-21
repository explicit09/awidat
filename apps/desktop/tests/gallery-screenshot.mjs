import { chromium } from "playwright";

const url = process.env.GALLERY_URL ?? "http://localhost:1420/gallery.html";
const outPath = process.env.GALLERY_OUT ?? "tests/gallery.png";

const browser = await chromium.launch();
const ctx = await browser.newContext({
  viewport: { width: 1400, height: 1000 },
  deviceScaleFactor: 2,
});
const page = await ctx.newPage();

const errors = [];
page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
page.on("console", (m) => {
  if (m.type() === "error" || m.type() === "warning") {
    errors.push(`${m.type()}: ${m.text()}`);
  }
});

const failedReqs = [];
page.on("requestfailed", (r) => failedReqs.push(`${r.url()} — ${r.failure()?.errorText}`));
page.on("response", (r) => {
  if (r.status() >= 400) failedReqs.push(`${r.status()} ${r.url()}`);
});

await page.goto(url, { waitUntil: "networkidle" });
await page.waitForTimeout(400);
await page.screenshot({ path: outPath, fullPage: true });

await browser.close();

if (errors.length || failedReqs.length) {
  if (errors.length) console.error("Errors:\n" + errors.join("\n"));
  if (failedReqs.length) console.error("Failed requests:\n" + failedReqs.join("\n"));
  process.exit(1);
}
console.log(`OK — wrote ${outPath}`);
