import { summarizeJobs, jobKindLabel } from "../src/shell/jobsSummary";

function assert(cond: unknown, msg?: string): asserts cond {
  if (!cond) throw new Error(msg ?? "assertion failed");
}

assert(summarizeJobs([]) === "Idle", "no jobs -> Idle");

const summary = summarizeJobs([
  { id: "j1", kind: "transcode", percent: 40, status: "" },
  { id: "j2", kind: "transcode", percent: 0, status: "" },
  { id: "j3", kind: "indexing", percent: 60, status: "" },
]);
assert(summary === "2 transcoding, 1 indexing", `got: ${summary}`);

const single = summarizeJobs([
  { id: "j1", kind: "thumbnails", percent: 10, status: "" },
]);
assert(single === "1 thumbnailing", `got: ${single}`);

assert(jobKindLabel("transcode") === "transcoding");
assert(jobKindLabel("indexing") === "indexing");
assert(jobKindLabel("thumbnails") === "thumbnailing");
assert(jobKindLabel("waveform") === "waveform");
assert(jobKindLabel("render") === "rendering");
assert(jobKindLabel("unknown_kind") === "unknown_kind", "fallback unmapped kinds");

console.log("ok  jobs summary + labels");
