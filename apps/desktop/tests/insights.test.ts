import assert from "node:assert/strict";
import { onTimelineSpan, timelineTimeForSourceTime } from "../src/timeline/sourceTimeMap.ts";
import {
  detectFillerMoments,
  detectSilenceMoments,
  estimatedSavingsS,
} from "../src/shell/insights.ts";

// ── source → timeline mapping ──────────────────────────────────────
const segments = [
  // First minute of the source plays at the head of the timeline.
  { proxyStem: "ep1", sourceStem: "raw1", sourceStart: 0, sourceEnd: 60, timelineStart: 0, speed: 1 },
  // 90s..120s of source follows directly (30s of source was cut).
  { proxyStem: "ep1", sourceStem: "raw1", sourceStart: 90, sourceEnd: 120, timelineStart: 60, speed: 1 },
  // A 2x-speed segment of a second asset.
  { proxyStem: "ep2", sourceStem: null, sourceStart: 10, sourceEnd: 30, timelineStart: 90, speed: 2 },
];

assert.equal(timelineTimeForSourceTime(segments, "ep1", 15), 15);
assert.equal(timelineTimeForSourceTime(segments, "raw1", 15), 15, "matches sourceStem too");
assert.equal(timelineTimeForSourceTime(segments, "ep1", 100), 70, "second segment offsets");
assert.equal(timelineTimeForSourceTime(segments, "ep1", 75), null, "cut-out material drops");
assert.equal(timelineTimeForSourceTime(segments, "ep2", 20), 95, "speed compresses timeline advance");
assert.equal(timelineTimeForSourceTime(segments, "nope", 15), null);
assert.equal(timelineTimeForSourceTime(segments, "ep1", Number.NaN), null);

// ── on-timeline overlap (partial cuts shrink moments) ──────────────
// Silence spanning 55..59 in source; the 60..90 cut doesn't touch it.
assert.deepEqual(onTimelineSpan(segments, "ep1", 55, 59), { overlapS: 4, firstSourceS: 55 });
// Silence 58..63: only 58..60 survives (2s of 5s) — start still mapped.
assert.deepEqual(onTimelineSpan(segments, "ep1", 58, 63), { overlapS: 2, firstSourceS: 58 });
// Silence 70..80 entirely inside the cut: gone.
assert.equal(onTimelineSpan(segments, "ep1", 70, 80), null);
// Silence 85..95: head cut off, tail survives from 90.
assert.deepEqual(onTimelineSpan(segments, "ep1", 85, 95), { overlapS: 5, firstSourceS: 90 });
// Source seconds in a 2x segment occupy half as much timeline time.
assert.deepEqual(onTimelineSpan(segments, "ep2", 10, 14), { overlapS: 2, firstSourceS: 10 });
assert.equal(onTimelineSpan(segments, "ep1", 59, 59), null, "empty span");

// ── filler detection ───────────────────────────────────────────────
const words = [
  { text: "Um,", start_s: 1, end_s: 1.4 },
  { text: "today", start_s: 1.4, end_s: 1.8 },
  { text: "we", start_s: 1.8, end_s: 2.0 },
  { text: "you", start_s: 2.0, end_s: 2.2 },
  { text: "know,", start_s: 2.2, end_s: 2.5 },
  { text: "ship", start_s: 2.5, end_s: 2.9 },
  { text: "like", start_s: 2.9, end_s: 3.1 }, // bare "like" must NOT count
];
const fillers = detectFillerMoments(words, "ep1");
assert.equal(fillers.length, 2);
assert.equal(fillers[0].sourceTimeS, 1);
assert.equal(fillers[0].detail, "“Um,”");
assert.equal(fillers[1].detail, "“you know,”");
assert.ok(Math.abs(fillers[1].durationS - 0.5) < 1e-9);

// ── silence detection + savings ────────────────────────────────────
const silences = detectSilenceMoments(
  [
    { start_s: 5, duration_s: 4.2 },
    { start_s: 30, duration_s: 1.0 }, // under threshold → dropped
    { start_s: 50, duration_s: 2.5 },
  ],
  "ep1",
  2,
);
assert.equal(silences.length, 2);
assert.equal(silences[0].label, "Silence (4.2s)");
// savings keep a 0.3s beat per silence: (4.2-0.3) + (2.5-0.3) = 6.1
assert.ok(Math.abs(estimatedSavingsS(silences) - 6.1) < 1e-9);
// fillers reclaim their full span: 0.4 + 0.5 = 0.9
assert.ok(Math.abs(estimatedSavingsS(fillers) - 0.9) < 1e-9);

console.log("insights ok");
