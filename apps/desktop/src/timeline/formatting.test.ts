import assert from "node:assert/strict";
import {
  formatBadgeNumber,
  formatTime,
  transitionLabel,
} from "./formatting.ts";
import { computePps } from "./layout.ts";

console.log("# computePps");
assert.equal(computePps(60, 728, 1), 11.133333333333333);
assert.equal(computePps(2, 1200, 4), 96);
assert.equal(computePps(0, 500, 1), 12);
console.log("  ok  fits duration to width and caps zoomed pixels-per-second");

console.log("# transitionLabel");
assert.equal(transitionLabel("awidat.cross_dissolve"), "Dissolve");
assert.equal(transitionLabel("awidat.flash_white"), "Flash");
assert.equal(transitionLabel("custom_wipe"), "custom wipe");
console.log("  ok  maps known effects and prettifies fallback names");

console.log("# formatBadgeNumber");
assert.equal(formatBadgeNumber(0.5), "0.5");
assert.equal(formatBadgeNumber(2), "2");
assert.equal(formatBadgeNumber(1.25), "1.25");
console.log("  ok  trims multiplier decimals");

console.log("# formatTime");
assert.equal(formatTime(0), "0:00");
assert.equal(formatTime(65.9), "1:05");
assert.equal(formatTime(Number.NaN), "0:00");
console.log("  ok  formats timeline seconds as m:ss");
