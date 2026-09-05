#!/usr/bin/env node
/** Build and exercise the minified Vite renderer harness without Tauri packaging. */

import { createHash } from "node:crypto";
import { spawn, execFileSync } from "node:child_process";
import { lstat, mkdtemp, mkdir, readFile, readdir, realpath, rename, rm, writeFile } from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";

const DESKTOP_DIR = fileURLToPath(new URL("..", import.meta.url));
const WORKSPACE_ROOT = resolve(DESKTOP_DIR, "../..");
const LABEL = process.env.PERF_LABEL;
const PORT = Number(process.env.PERF_PORT ?? 4173);
const RUNS = Number(process.env.PERF_RUNS ?? 3);
const SUITE = process.env.PERF_SUITE ?? "full";
const safeLabel = LABEL?.replace(/[^a-zA-Z0-9._-]/g, "-");
const defaultOutput = join(
  os.tmpdir(),
  `montage-desktop-production-perf-${safeLabel ?? "missing"}-${process.pid}`,
);
const defaultEvidence = join(
  os.tmpdir(),
  "montage-desktop-production-perf",
  "evidence",
);
const OUTPUT_DIR = resolve(process.env.PERF_OUTPUT_DIR ?? defaultOutput);
const EVIDENCE_DIR = resolve(process.env.PERF_EVIDENCE_DIR ?? defaultEvidence);
const BASE_URL = `http://127.0.0.1:${PORT}/`;
const SLEEP_PREVENTION_ARGS = process.platform === "darwin"
  ? ["-disu", "-w", String(process.pid)]
  : null;
const SOURCE_PROVENANCE_PATHS = [
  "apps/desktop/tests/ui-harness.html",
  "apps/desktop/vite.config.ts",
  "apps/desktop/tests/perf-production.mjs",
  "apps/desktop/tests/perf-full.mjs",
  "apps/desktop/tests/perf-playback.mjs",
  "apps/desktop/src/App.tsx",
  "apps/desktop/src/timeline/TimelineSurface.tsx",
];
const TIMELINE_PAINT_SENTINELS = [
  "__montageTimelinePaintMetrics",
  "__montageTimelinePaintInstrumentationVersion",
];
const NORMAL_HTML_INPUTS = ["index.html"];

function fail(message) {
  throw new Error(message);
}

function commandText(command, args) {
  return [command, ...args.map((arg) => (arg.includes(" ") ? JSON.stringify(arg) : arg))].join(" ");
}

function commandOutput(command, args) {
  try {
    return execFileSync(command, args, { cwd: WORKSPACE_ROOT, encoding: "utf8" }).trim();
  } catch {
    return null;
  }
}

function run(command, args, options = {}) {
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(command, args, { cwd: WORKSPACE_ROOT, stdio: "inherit", ...options });
    child.once("error", rejectRun);
    child.once("exit", (code, signal) => resolveRun({ code, signal }));
  });
}

async function startSleepPrevention() {
  if (!SLEEP_PREVENTION_ARGS) return null;
  const child = spawn("caffeinate", SLEEP_PREVENTION_ARGS, {
    cwd: WORKSPACE_ROOT,
    stdio: "ignore",
  });
  await new Promise((resolveSpawn, rejectSpawn) => {
    child.once("error", rejectSpawn);
    child.once("spawn", resolveSpawn);
  });
  return child;
}

function stopSleepPrevention(child) {
  if (child?.exitCode === null) child.kill("SIGTERM");
}

async function requireFreePort() {
  await new Promise((resolvePort, rejectPort) => {
    const server = net.createServer();
    server.once("error", (error) => rejectPort(new Error(`port ${PORT} is unavailable: ${error.message}`)));
    server.listen(PORT, "127.0.0.1", () => server.close(resolvePort));
  });
}

async function waitForPreview(child) {
  for (let attempt = 0; attempt < 80; attempt += 1) {
    try {
      const response = await fetch(new URL("tests/ui-harness.html", BASE_URL), { signal: AbortSignal.timeout(500) });
      if (response.ok) return;
    } catch {}
    if (child.exitCode !== null) fail(`vite preview exited before ${BASE_URL} became reachable`);
    await delay(250);
  }
  fail(`timed out waiting for vite preview at ${BASE_URL}`);
}

async function stopChild(child) {
  if (!child || child.exitCode !== null) return;
  const stopped = new Promise((resolveStop) => child.once("exit", resolveStop));
  const kill = (signal) => {
    try {
      if (process.platform === "win32") child.kill(signal);
      else process.kill(-child.pid, signal);
    } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
  };
  kill("SIGTERM");
  const stoppedGracefully = await Promise.race([stopped.then(() => true), delay(2_000).then(() => false)]);
  if (!stoppedGracefully && child.exitCode === null) {
    kill("SIGKILL");
    await Promise.race([stopped, delay(2_000)]);
  }
}

async function filesUnder(path) {
  const entries = await readdir(path, { withFileTypes: true });
  const files = await Promise.all(entries.map(async (entry) => {
    const entryPath = join(path, entry.name);
    return entry.isDirectory() ? filesUnder(entryPath) : [entryPath];
  }));
  return files.flat();
}

async function hashFile(path) {
  return createHash("sha256").update(await readFile(path)).digest("hex");
}

async function hashEntries(paths) {
  return Promise.all(paths.map(async (path) => ({ path, sha256: await hashFile(path) })));
}

async function filesContaining(paths, needle) {
  const matches = [];
  for (const path of paths) {
    if ((await readFile(path, "utf8")).includes(needle)) matches.push(path);
  }
  return matches;
}

async function existingParent(path) {
  let candidate = path;
  while (true) {
    try {
      await lstat(candidate);
      return candidate;
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
      const parent = dirname(candidate);
      if (parent === candidate) fail(`no existing parent for PERF_OUTPUT_DIR: ${OUTPUT_DIR}`);
      candidate = parent;
    }
  }
}

function plistValue(plist, key) {
  const match = plist.match(new RegExp(`<key>${key}</key>\\s*<string>([^<]+)</string>`));
  return match?.[1] ?? null;
}

async function requireApfsOutput() {
  if (process.platform !== "darwin") fail("production performance staging requires macOS APFS");
  const stagingParent = await existingParent(OUTPUT_DIR);
  const df = execFileSync("df", ["-P", stagingParent], { encoding: "utf8" }).trim().split("\n").at(-1).trim().split(/\s+/);
  const device = df[0];
  const diskInfo = execFileSync("diskutil", ["info", "-plist", device], { encoding: "utf8" });
  const filesystem = plistValue(diskInfo, "FilesystemType");
  const deviceIdentifier = plistValue(diskInfo, "DeviceIdentifier") ?? device.replace("/dev/", "");
  if (filesystem?.toLowerCase() !== "apfs") fail(`PERF_OUTPUT_DIR staging filesystem must be APFS, got ${filesystem ?? "unknown"} on ${deviceIdentifier}`);
  return { stagingParent, filesystem, device: deviceIdentifier };
}

async function writeAtomic(path, content) {
  const temporary = `${path}.${process.pid}.tmp`;
  await writeFile(temporary, content);
  await rename(temporary, path);
}

async function requireMissingOutput() {
  try {
    await lstat(OUTPUT_DIR);
    fail(`PERF_OUTPUT_DIR must not already exist: ${OUTPUT_DIR}`);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
}

async function requireOutputOutsideWorktree() {
  const parent = await existingParent(OUTPUT_DIR);
  const [physicalParent, physicalWorkspace] = await Promise.all([
    realpath(parent),
    realpath(WORKSPACE_ROOT),
  ]);
  const physicalOutput = resolve(physicalParent, relative(parent, OUTPUT_DIR));
  const relation = relative(physicalWorkspace, physicalOutput);
  const insideWorktree = relation === ""
    || (relation !== ".." && !relation.startsWith(`..${sep}`) && !isAbsolute(relation));
  if (insideWorktree) fail("PERF_OUTPUT_DIR must be outside this worktree");
}

if (!LABEL || !safeLabel) fail("PERF_LABEL is required (use a filesystem-safe label)");
if (!Number.isInteger(PORT) || PORT < 1 || PORT > 65535) fail("PERF_PORT must be a valid TCP port");
if (!Number.isInteger(RUNS) || RUNS < 1) fail("PERF_RUNS must be a positive integer");
if (SUITE !== "full" && SUITE !== "playback") fail("PERF_SUITE must be full or playback");
if (OUTPUT_DIR === WORKSPACE_ROOT || OUTPUT_DIR.startsWith(`${WORKSPACE_ROOT}/`)) fail("PERF_OUTPUT_DIR must be outside this worktree");

const buildArgs = ["--dir", "apps/desktop", "exec", "vite", "build", "--mode", "perf", "--outDir", OUTPUT_DIR];
const previewArgs = ["--dir", "apps/desktop", "exec", "vite", "preview", "--host", "127.0.0.1", "--port", String(PORT), "--strictPort", "--outDir", OUTPUT_DIR];
const typecheckArgs = ["--dir", "apps/desktop", "typecheck"];
const buildCommand = commandText("pnpm", buildArgs);
const previewCommand = commandText("pnpm", previewArgs);
const typecheckCommand = commandText("pnpm", typecheckArgs);
const fixturePath = join(DESKTOP_DIR, "tests/fixtures/perf-preview.mp4");
let preview = null;
let sleepPreventer = null;
let normalOutputDir = null;
let stopping = false;

async function stopForSignal(code) {
  if (stopping) return;
  stopping = true;
  await stopChild(preview);
  stopSleepPrevention(sleepPreventer);
  if (normalOutputDir) await rm(normalOutputDir, { recursive: true, force: true });
  process.exit(code);
}

process.on("SIGINT", () => { void stopForSignal(130); });
process.on("SIGTERM", () => { void stopForSignal(143); });

try {
  await requireMissingOutput();
  await requireOutputOutsideWorktree();
  sleepPreventer = await startSleepPrevention();
  const outputFilesystem = await requireApfsOutput();
  await requireFreePort();

  const typecheckResult = await run("pnpm", typecheckArgs);
  if (typecheckResult.code !== 0) fail(`desktop typecheck failed (${typecheckResult.code ?? typecheckResult.signal})`);

  normalOutputDir = await mkdtemp(join(os.tmpdir(), "montage-desktop-normal-dce-"));
  const normalBuildArgs = ["--dir", "apps/desktop", "exec", "vite", "build", "--outDir", normalOutputDir];
  const normalBuildCommand = commandText("pnpm", normalBuildArgs);
  const normalBuildResult = await run("pnpm", normalBuildArgs);
  if (normalBuildResult.code !== 0) fail(`normal Vite build failed (${normalBuildResult.code ?? normalBuildResult.signal})`);
  const normalFiles = await filesUnder(normalOutputDir);
  const normalHtmlInputs = normalFiles
    .filter((path) => path.endsWith(".html"))
    .map((path) => path.slice(normalOutputDir.length + 1))
    .sort();
  if (JSON.stringify(normalHtmlInputs) !== JSON.stringify(NORMAL_HTML_INPUTS)) {
    fail(`normal build HTML inputs changed: ${normalHtmlInputs.join(", ")}`);
  }
  if (normalFiles.some((path) => path.endsWith("tests/ui-harness.html") || /perf-preview-.*\.mp4$/.test(path))) {
    fail("normal build contains a perf-only harness or media fixture");
  }
  const normalJavascript = normalFiles.filter((path) => path.endsWith(".js"));
  const normalSentinelMatches = (
    await Promise.all(
      TIMELINE_PAINT_SENTINELS.map((sentinel) => filesContaining(normalJavascript, sentinel)),
    )
  ).flat();
  if (normalSentinelMatches.length !== 0) fail("timeline paint instrumentation leaked into the normal build");

  const buildResult = await run("pnpm", buildArgs);
  if (buildResult.code !== 0) fail(`perf Vite build failed (${buildResult.code ?? buildResult.signal})`);

  const harnessPath = join(OUTPUT_DIR, "tests/ui-harness.html");
  const builtFiles = await filesUnder(OUTPUT_DIR);
  const mediaPaths = builtFiles.filter((path) => /^perf-preview-[A-Za-z0-9_-]{8,}\.mp4$/.test(basename(path)));
  if (!builtFiles.includes(harnessPath)) fail(`missing production harness: ${harnessPath}`);
  if (mediaPaths.length !== 1) fail(`expected exactly one hashed perf-preview MP4 asset, found ${mediaPaths.length}`);
  const perfJavascript = builtFiles.filter((path) => path.endsWith(".js"));
  const perfSentinelMatchesByName = await Promise.all(
    TIMELINE_PAINT_SENTINELS.map(async (sentinel) => ({
      sentinel,
      matches: await filesContaining(perfJavascript, sentinel),
    })),
  );
  const missingPerfSentinels = perfSentinelMatchesByName
    .filter(({ matches }) => matches.length === 0)
    .map(({ sentinel }) => sentinel);
  if (missingPerfSentinels.length > 0) {
    fail(`perf build is missing timeline paint instrumentation: ${missingPerfSentinels.join(", ")}`);
  }
  const perfSentinelMatches = [
    ...new Set(perfSentinelMatchesByName.flatMap(({ matches }) => matches)),
  ];
  const fixtureSha256 = await hashFile(fixturePath);
  const emittedMedia = await hashEntries(mediaPaths);
  if (emittedMedia[0].sha256 !== fixtureSha256) fail("emitted perf-preview MP4 SHA-256 does not match source fixture");

  await mkdir(EVIDENCE_DIR, { recursive: true });
  const stageDir = await mkdtemp(join(EVIDENCE_DIR, ".staging-"));
  const evidenceName = `${safeLabel}-${new Date().toISOString().replace(/[:.]/g, "-")}-${process.pid}`;
  const evidencePath = join(EVIDENCE_DIR, evidenceName);
  const emitted = (extension) => builtFiles.filter((path) => path.endsWith(extension));
  const buildEvidence = {
    label: LABEL,
    generatedAt: new Date().toISOString(),
    build: {
      mode: "perf",
      command: buildCommand,
      typecheck: { command: typecheckCommand, passed: true },
      outputDir: OUTPUT_DIR,
      outputFilesystem,
      harness: { path: harnessPath, sha256: await hashFile(harnessPath) },
      timelinePaintSentinelPresent: true,
      timelinePaintSentinelFiles: perfSentinelMatches.map((path) => path.slice(OUTPUT_DIR.length + 1)),
    },
    normalBuildDce: {
      command: normalBuildCommand,
      htmlInputs: normalHtmlInputs,
      perfArtifactsAbsent: true,
      timelinePaintSentinelAbsent: true,
    },
    fixture: { path: fixturePath, sha256: fixtureSha256, emittedAssetVerified: true },
    sourceProvenance: await Promise.all(SOURCE_PROVENANCE_PATHS.map(async (path) => ({ path, sha256: await hashFile(join(WORKSPACE_ROOT, path)) }))),
    emittedAssets: {
      javascript: await hashEntries(emitted(".js")),
      css: await hashEntries(emitted(".css")),
      media: emittedMedia,
    },
    repository: {
      worktree: WORKSPACE_ROOT,
      head: commandOutput("git", ["rev-parse", "HEAD"]),
      branch: commandOutput("git", ["branch", "--show-current"]),
      dirty: commandOutput("git", ["status", "--porcelain"]),
    },
    environment: {
      node: process.version,
      pnpm: commandOutput("pnpm", ["--version"]),
      vite: commandOutput("pnpm", ["--dir", "apps/desktop", "exec", "vite", "--version"]),
      os: `${os.type()} ${os.release()}`,
      arch: process.arch,
      browser: "Chromium via Playwright (version recorded by each perf-full run)",
      sleepPrevention: SLEEP_PREVENTION_ARGS
        ? commandText("caffeinate", SLEEP_PREVENTION_ARGS)
        : null,
    },
    preview: { command: previewCommand, mode: "production-minified", port: PORT, baseUrl: BASE_URL },
  };
  const stagedEvidence = join(stageDir, "build-evidence.json");
  await writeAtomic(stagedEvidence, `${JSON.stringify(buildEvidence, null, 2)}\n`);
  await rename(stageDir, evidencePath);
  const buildEvidencePath = join(evidencePath, "build-evidence.json");
  const buildEvidenceSha256 = await hashFile(buildEvidencePath);

  preview = spawn("pnpm", previewArgs, {
    cwd: WORKSPACE_ROOT,
    env: { ...process.env, BROWSER: "none" },
    detached: process.platform !== "win32",
    stdio: ["ignore", "pipe", "pipe"],
  });
  preview.stdout.on("data", (chunk) => process.stdout.write(chunk));
  preview.stderr.on("data", (chunk) => process.stderr.write(chunk));
  await waitForPreview(preview);

  const runs = [];
  const runCount = SUITE === "playback" ? 1 : RUNS;
  for (let index = 1; index <= runCount; index += 1) {
    const playback = SUITE === "playback";
    const runLabel = playback ? safeLabel : `${safeLabel}-${index}`;
    const script = playback ? "apps/desktop/tests/perf-playback.mjs" : "apps/desktop/tests/perf-full.mjs";
    const runCommand = `PERF_LABEL=${runLabel} PERF_URL=${BASE_URL} node ${script}`;
    const result = await run("node", [script], {
      env: {
        ...process.env,
        PERF_LABEL: runLabel,
        PERF_URL: BASE_URL,
        PERF_OUT_DIR: evidencePath,
        PERF_PLAYBACK_OUT_DIR: evidencePath,
        PERF_SERVER_MODE: "production-minified",
        PERF_BUILD_EVIDENCE_PATH: buildEvidencePath,
        PERF_BUILD_EVIDENCE_SHA256: buildEvidenceSha256,
        PERF_BUILD_COMMAND: buildCommand,
        PERF_SERVER_COMMAND: previewCommand,
        PERF_RUN_COMMAND: runCommand,
      },
    });
    runs.push({
      label: runLabel,
      suite: SUITE,
      exitCode: result.code,
      signal: result.signal,
      report: join(evidencePath, playback ? `desktop-playback-performance-${runLabel}.json` : `desktop-ux-performance-${runLabel}.json`),
    });
  }
  await writeAtomic(join(evidencePath, "runs.json"), `${JSON.stringify({ label: LABEL, suite: SUITE, outputDir: OUTPUT_DIR, buildEvidencePath, runs }, null, 2)}\n`);
  console.log(`\nBuild evidence: ${buildEvidencePath}`);
  console.log(`Run evidence: ${evidencePath}`);
  console.log(`Retained build output: ${OUTPUT_DIR}`);
  if (runs.some((runResult) => runResult.exitCode !== 0)) process.exitCode = 1;
} finally {
  await stopChild(preview);
  stopSleepPrevention(sleepPreventer);
  if (normalOutputDir) await rm(normalOutputDir, { recursive: true, force: true });
}
