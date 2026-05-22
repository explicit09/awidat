import { chromium } from "playwright";

const browser = await chromium.launch();
const ctx = await browser.newContext({
  viewport: { width: 1400, height: 1000 },
  deviceScaleFactor: 2,
});

async function snap(stage, outPath) {
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(`console.error: ${m.text()}`);
  });
  await page.goto("http://localhost:1420/", { waitUntil: "networkidle" });
  // Click the stage button by aria-selected falsy and matching name.
  await page.evaluate((s) => {
    // Reach into the running app and bump useStageStore.
    // Stage buttons are role=tab with their label inside.
    // Easier: dispatch via DOM click.
    const btns = Array.from(document.querySelectorAll('button[role="tab"]'));
    const btn = btns.find((b) => b.textContent?.trim().toLowerCase() === s.toLowerCase());
    if (btn) (btn).click();
  }, stage);
  await page.waitForTimeout(300);
  await page.screenshot({ path: outPath, fullPage: false });
  if (errors.length) {
    console.error(`Errors on stage=${stage}:`);
    console.error(errors.join("\n"));
  }
  await page.close();
}

await snap("Edit", "tests/app-edit.png");
await snap("Deliver", "tests/app-deliver.png");

await browser.close();
console.log("OK - wrote 2 stage screenshots to tests/");
