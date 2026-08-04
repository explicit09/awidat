import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { lstat, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import os from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("./perf-production.mjs", import.meta.url));
const workspace = fileURLToPath(new URL("../../..", import.meta.url));
const source = await readFile(script, "utf8");

assert.match(source, /const defaultOutput = join\(\s*os\.tmpdir\(\)/);
assert.match(source, /const defaultEvidence = join\(\s*os\.tmpdir\(\)/);
assert.doesNotMatch(source, /\/Volumes\/My Passport for Mac/);
assert.doesNotMatch(source, /\/private\/tmp/);
assert.match(source, /process\.env\.PERF_OUTPUT_DIR \?\? defaultOutput/);
assert.match(source, /process\.env\.PERF_EVIDENCE_DIR \?\? defaultEvidence/);

const root = await mkdtemp(join(os.tmpdir(), "montage-perf-path-guard-"));
try {
  const alias = join(root, "worktree-link");
  const stubBin = join(root, "bin");
  const output = join(alias, `missing-output-${process.pid}`);
  await symlink(workspace, alias, process.platform === "win32" ? "junction" : "dir");
  await mkdir(stubBin);
  await writeFile(join(stubBin, "pnpm"), "#!/bin/sh\nexit 77\n", { mode: 0o755 });

  const result = spawnSync(process.execPath, [script], {
    cwd: workspace,
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${stubBin}${delimiter}${process.env.PATH ?? ""}`,
      PERF_EVIDENCE_DIR: join(root, "evidence"),
      PERF_LABEL: "path-guard-test",
      PERF_OUTPUT_DIR: output,
      PERF_PORT: "41737",
    },
    timeout: 10_000,
  });

  assert.notEqual(result.status, 0);
  assert.match(
    `${result.stdout}${result.stderr}`,
    /PERF_OUTPUT_DIR must be outside this worktree/,
  );
  await assert.rejects(lstat(output), { code: "ENOENT" });
} finally {
  await rm(root, { recursive: true, force: true });
}

console.log("perf-production paths ok");
