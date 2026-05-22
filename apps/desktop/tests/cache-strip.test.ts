import { cacheStripSegments } from "../src/timeline/cacheStrip";

function assert(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Error(msg);
}
function deepEq<T>(a: T, b: T, msg: string): asserts a is T {
  if (JSON.stringify(a) !== JSON.stringify(b)) {
    throw new Error(`${msg}: got ${JSON.stringify(a)} want ${JSON.stringify(b)}`);
  }
}

// Groups consecutive same-kind ranges.
const merged = cacheStripSegments([
  { startS: 0, endS: 5, kind: "proxy" },
  { startS: 5, endS: 10, kind: "proxy" },
  { startS: 10, endS: 14, kind: "source" },
  { startS: 14, endS: 18, kind: "missing" },
]);
assert(merged.length === 3, `merged length ${merged.length}`);
deepEq(merged[0], { startS: 0, endS: 10, kind: "proxy" }, "first segment");
deepEq(merged[1], { startS: 10, endS: 14, kind: "source" }, "second segment");
deepEq(merged[2], { startS: 14, endS: 18, kind: "missing" }, "third segment");

// Does NOT merge when there's a gap.
const gapped = cacheStripSegments([
  { startS: 0, endS: 4, kind: "proxy" },
  { startS: 6, endS: 10, kind: "proxy" },
]);
assert(gapped.length === 2, "gap breaks the run");

// Does NOT merge when kind differs even if abutting.
const flips = cacheStripSegments([
  { startS: 0, endS: 5, kind: "proxy" },
  { startS: 5, endS: 10, kind: "source" },
]);
assert(flips.length === 2, "kind change keeps separate segments");

// Empty input -> empty output.
assert(cacheStripSegments([]).length === 0, "empty input");

console.log("ok  cache strip merger");
