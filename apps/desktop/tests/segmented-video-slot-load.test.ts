import {
  beginSlotLoad,
  isCurrentSlotLoad,
  mediaHasFutureData,
  shouldStartMediaPlayback,
  type SegmentSlotLoadState,
} from "../src/media/segmentedVideoSlotLoad";

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const slot: SegmentSlotLoadState = {
  loadToken: 0,
  wantedPath: null,
};

const first = beginSlotLoad(slot, "/project/raw/a.mov");
const second = beginSlotLoad(slot, "/project/raw/b.mov");

assert(
  !isCurrentSlotLoad(slot, first),
  "older async slot load must be stale after a newer path is requested",
);
assert(
  isCurrentSlotLoad(slot, second),
  "latest async slot load must still be accepted",
);

assert(
  !mediaHasFutureData(2),
  "loaded metadata alone is not enough to start timeline playback",
);
assert(
  mediaHasFutureData(3),
  "HAVE_FUTURE_DATA should be enough to start timeline playback",
);

assert(
  !shouldStartMediaPlayback({ isPlaying: true, paused: true, readyState: 2 }),
  "overlay playback must also wait for future data",
);
assert(
  shouldStartMediaPlayback({ isPlaying: true, paused: true, readyState: 3 }),
  "ready paused media should start when preview playback is active",
);
assert(
  !shouldStartMediaPlayback({ isPlaying: true, paused: false, readyState: 4 }),
  "already-playing media should not receive redundant play calls",
);

console.log("ok  segmented video slot load guards reject stale completions");
