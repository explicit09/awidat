import { strict as assert } from "node:assert";
import { frozenClock, livePreviewClock } from "../src/media/stage/stageClock.ts";

const f = frozenClock(12.5);
assert.equal(f.now(), 12.5);
assert.equal(f.isPlaying(), false);
assert.equal(f.rate(), 0);

let t = 3;
const live = livePreviewClock({ now: () => t, isPlaying: () => true, rate: () => 1.5 });
assert.equal(live.now(), 3);
t = 4;
assert.equal(live.now(), 4);
assert.equal(live.isPlaying(), true);
assert.equal(live.rate(), 1.5);

console.log("stage-clock: OK");
