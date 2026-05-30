// Tests for the evidence drill-down helpers (Wave 3 C1).
//
// The drill-down components themselves are React markup and live behind
// the renderer; we lock in their pure helpers — timecode formatting and
// the small jump helper's defensive coercion — so refactors that change
// the visual surface don't silently break the time labels users read.

import { strict as assert } from "node:assert";
import { formatTimecode } from "../src/shell/brief/drillDowns/utils.ts";

// formatTimecode — sub-minute → `m:ss`
{
  assert.equal(formatTimecode(0), "0:00");
  assert.equal(formatTimecode(5), "0:05");
  assert.equal(formatTimecode(59), "0:59");
}

// formatTimecode — sub-hour → `m:ss`
{
  assert.equal(formatTimecode(60), "1:00");
  assert.equal(formatTimecode(125), "2:05");
  assert.equal(formatTimecode(3599), "59:59");
}

// formatTimecode — hour-plus → `h:mm:ss`
{
  assert.equal(formatTimecode(3600), "1:00:00");
  assert.equal(formatTimecode(3661), "1:01:01");
  assert.equal(formatTimecode(36000), "10:00:00");
}

// formatTimecode — defensive on bogus input
{
  assert.equal(formatTimecode(-5), "0:00");
  assert.equal(formatTimecode(Number.NaN), "0:00");
  assert.equal(formatTimecode(Infinity), "0:00");
}

// formatTimecode — fractional seconds floor to integer second
{
  assert.equal(formatTimecode(5.7), "0:05");
  assert.equal(formatTimecode(125.999), "2:05");
}

console.log("evidence-drill-downs: OK");
