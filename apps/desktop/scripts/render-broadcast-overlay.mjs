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
const fps = Number(process.env.MONTAGE_BROADCAST_OVERLAY_FPS ?? args.fps ?? 30);
const duration = Number(args.duration);
// Optional render window. `--time-offset T` shifts both the overlay's
// internal animation clock (frame N maps to overlay-time T + N/fps) and the
// encoded file's presentation timestamps (via ffmpeg -output_ts_offset), so a
// short windowed clip lands at the correct timeline position when composited.
// Used by view_program_frame to inspect one frame without rendering the whole
// episode's overlay (which is multi-GB and fills the disk).
const timeOffset = Number(args["time-offset"] ?? 0);

const projectRoot = args["project-root"];

if (!Number.isFinite(timeOffset) || timeOffset < 0) {
  console.error("--time-offset must be a non-negative number of seconds");
  process.exit(2);
}
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
  await page.route("**/__montage_asset__/**", async (route) => {
    const url = new URL(route.request().url());
    const marker = "/__montage_asset__/";
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
    window.__MONTAGE_OVERLAY_PAYLOAD__ = payload;
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
  await page.waitForFunction(() => window.__MONTAGE_OVERLAY_READY__ === true);
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
    // Shift encoded timestamps so a windowed clip aligns to its timeline
    // position; 0 for a full-episode render leaves PTS starting at 0.
    ...(timeOffset > 0 ? ["-output_ts_offset", String(timeOffset)] : []),
    output,
  ], { stdio: ["pipe", "inherit", "inherit"] });

  // Attach lifecycle listeners synchronously so spawn-time failures
  // (binary missing, immediate exit) propagate instead of crashing the
  // process or hanging the frame loop. `ffmpegDone` resolves after the
  // stdin pump finishes (either output complete or pump aborted by an
  // early ffmpeg exit) so the caller still gets a single await point.
  const ffmpegExit = waitForProcess(ffmpeg, "ffmpeg");
  let ffmpegFailure = null;
  ffmpegExit.catch((err) => { ffmpegFailure = err; });
  ffmpeg.stdin.on("error", (err) => { ffmpegFailure ??= err; });

  try {
    for (let frame = 0; frame < frameCount; frame += 1) {
      if (ffmpegFailure) break;
      const t = timeOffset + frame / fps;
      await page.evaluate((time) => window.__MONTAGE_SET_OVERLAY_TIME__?.(time), t);
      const png = await page.screenshot({
        omitBackground: true,
        animations: "allow",
      });
      if (ffmpegFailure) break;
      if (!ffmpeg.stdin.write(png)) {
        await once(ffmpeg.stdin, "drain");
      }
    }
  } finally {
    if (!ffmpeg.stdin.destroyed) ffmpeg.stdin.end();
  }
  await ffmpegExit;
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
