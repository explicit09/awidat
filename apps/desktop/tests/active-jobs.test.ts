import {
  toActiveJobLike,
  aggregatePercent,
  type RawJobItem,
} from "../src/shell/activeJobs";

function assert(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Error(msg);
}

// Adapter: maps Item::Job → ActiveJobLike. Missing percent collapses
// to 0; missing status to empty string.
const fromItem: RawJobItem = {
  id: "j1",
  job_kind: "transcode",
  status: "encoding…",
  percent: 42,
};
const adapted = toActiveJobLike(fromItem);
assert(adapted.id === "j1", "id passes through");
assert(adapted.kind === "transcode", "kind comes from job_kind");
assert(adapted.percent === 42, "percent passes through");
assert(adapted.status === "encoding…", "status passes through");

const missing: RawJobItem = { id: "j2", job_kind: "indexing", status: null, percent: null };
const adaptedMissing = toActiveJobLike(missing);
assert(adaptedMissing.percent === 0, "null percent collapses to 0");
assert(adaptedMissing.status === "", "null status collapses to empty string");

// Aggregate: mean of non-NaN percents; null when no jobs.
assert(aggregatePercent([]) === null, "empty list -> null");
assert(
  aggregatePercent([
    { id: "a", kind: "transcode", percent: 20, status: "" },
    { id: "b", kind: "transcode", percent: 60, status: "" },
  ]) === 40,
  "two jobs at 20/60 -> 40",
);

console.log("ok  active-jobs adapter shape + aggregation");
