#!/usr/bin/env node
import { spawn } from "node:child_process";
import { once } from "node:events";
import { access, readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import { createServer } from "vite";

const args = parseArgs(process.argv.slice(2));
const desktopRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const width = Number(args.width ?? 1920);
const height = Number(args.height ?? 1080);
const fps = Number(process.env.AWIDAT_BROADCAST_OVERLAY_FPS ?? args.fps ?? 30);
const duration = Number(args.duration);

const projectRoot = args["project-root"];

if (!args.config || !projectRoot || !args.output || !Number.isFinite(duration) || duration <= 0) {
  console.error("usage: render-broadcast-overlay --config <json> --project-root <path> --duration <seconds> --output <mov> [--width 1920] [--height 1080] [--fps 30]");
  process.exit(2);
}

const overlay = JSON.parse(args.config);
const output = path.resolve(args.output);
let server;
let browser;

try {
  await assertOverlayAssetsExist(overlay, projectRoot);
  server = await createServer({
    root: desktopRoot,
    appType: "mpa",
    logLevel: "error",
    server: { host: "127.0.0.1", port: 0 },
  });
  await server.listen();
  const baseUrl = server.resolvedUrls?.local?.[0];
  if (!baseUrl) throw new Error("vite server did not expose a local URL");

  browser = await chromium.launch();
  const page = await browser.newPage({
    viewport: { width, height },
    deviceScaleFactor: 1,
  });
  await page.route("**/__awidat_asset__/**", async (route) => {
    const url = new URL(route.request().url());
    const marker = "/__awidat_asset__/";
    const markerIndex = url.pathname.indexOf(marker);
    const relPath = markerIndex >= 0 ? url.pathname.slice(markerIndex + marker.length) : "";
    const decoded = relPath
      .split("/")
      .map((part) => decodeURIComponent(part))
      .join("/");
    if (!decoded || decoded.startsWith("/") || decoded.includes("..")) {
      await route.abort();
      return;
    }
    const absolute = path.resolve(projectRoot, decoded);
    try {
      const body = await readFile(absolute);
      await route.fulfill({
        status: 200,
        contentType: contentTypeForPath(absolute),
        body,
      });
    } catch {
      await route.abort();
    }
  });
  await page.addInitScript(({ payload }) => {
    window.__AWIDAT_OVERLAY_PAYLOAD__ = payload;
  }, {
    payload: {
      overlay,
      projectRoot: path.resolve(projectRoot),
      width,
      height,
    },
  });
  await page.goto(new URL("overlay-render.html", baseUrl).toString(), {
    waitUntil: "networkidle",
  });
  await page.evaluate(() => {
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
    document.body.style.margin = "0";
    const root = document.getElementById("root");
    if (root) root.style.background = "transparent";
  });
  await page.waitForFunction(() => window.__AWIDAT_OVERLAY_READY__ === true);
  await page.waitForFunction(async () => {
    await document.fonts?.ready;
    const images = Array.from(document.images);
    await Promise.allSettled(
      images.map((image) => {
        if (image.complete) return Promise.resolve();
        return image.decode?.() ?? Promise.resolve();
      }),
    );
    return images.every((image) => image.complete && image.naturalWidth > 0);
  });

  const frameCount = Math.ceil(duration * fps);
  const ffmpeg = spawn("ffmpeg", [
    "-y",
    "-loglevel",
    "error",
    "-framerate",
    String(fps),
    "-f",
    "image2pipe",
    "-i",
    "pipe:0",
    "-c:v",
    "qtrle",
    "-pix_fmt",
    "argb",
    output,
  ], { stdio: ["pipe", "inherit", "inherit"] });
  for (let frame = 0; frame < frameCount; frame += 1) {
    const t = frame / fps;
    await page.evaluate((time) => window.__AWIDAT_SET_OVERLAY_TIME__?.(time), t);
    const png = await page.screenshot({
      omitBackground: true,
      animations: "allow",
    });
    if (!ffmpeg.stdin.write(png)) {
      await once(ffmpeg.stdin, "drain");
    }
  }
  ffmpeg.stdin.end();
  await waitForProcess(ffmpeg, "ffmpeg");
} finally {
  await browser?.close().catch(() => {});
  await server?.close().catch(() => {});
}

function contentTypeForPath(filePath) {
  const extension = path.extname(filePath).toLowerCase();
  if (extension === ".png") return "image/png";
  if (extension === ".jpg" || extension === ".jpeg") return "image/jpeg";
  if (extension === ".webp") return "image/webp";
  if (extension === ".svg") return "image/svg+xml";
  return "application/octet-stream";
}

async function assertOverlayAssetsExist(overlay, root) {
  const assetPaths = [
    overlay?.brand_logo_path,
    overlay?.host_a?.photo_path,
    overlay?.host_b?.photo_path,
  ].filter(Boolean);
  for (const relPath of assetPaths) {
    if (relPath.startsWith("/") || relPath.includes("..")) {
      throw new Error(`broadcast overlay asset path must be project-relative: ${relPath}`);
    }
    const absolute = path.resolve(root, relPath);
    await access(absolute).catch(() => {
      throw new Error(`broadcast overlay asset does not exist: ${absolute}`);
    });
  }
}

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (!arg.startsWith("--")) continue;
    const key = arg.slice(2);
    const next = argv[i + 1];
    if (!next || next.startsWith("--")) {
      out[key] = "true";
    } else {
      out[key] = next;
      i += 1;
    }
  }
  return out;
}

function waitForProcess(child, command) {
  return new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("exit", (code) => {
      if (code === 0) resolve();
      else reject(new Error(`${command} exited with ${code}`));
    });
  });
}
