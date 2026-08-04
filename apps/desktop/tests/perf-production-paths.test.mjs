import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const source = await readFile(fileURLToPath(new URL("./perf-production.mjs", import.meta.url)), "utf8");

assert.match(source, /const defaultOutput = join\(\s*os\.tmpdir\(\)/);
assert.match(source, /const defaultEvidence = join\(\s*os\.tmpdir\(\)/);
assert.doesNotMatch(source, /\/Volumes\/My Passport for Mac/);
assert.doesNotMatch(source, /\/private\/tmp/);
assert.match(source, /process\.env\.PERF_OUTPUT_DIR \?\? defaultOutput/);
assert.match(source, /process\.env\.PERF_EVIDENCE_DIR \?\? defaultEvidence/);

console.log("perf-production paths ok");
