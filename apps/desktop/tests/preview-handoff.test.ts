import { strict as assert } from "node:assert";
import {
  ENTRY_DRIFT_TOLERANCE_S,
  SHUTTLE_MIN_RATE,
  UNDERRUN_REBASE_MAX_S,
  driftRecoveryAction,
  fadeGainMultiplier,
  isShuttleRate,
  syncDriftThresholdS,
  timelineTimeForSegmentPosition,
} from "../src/media/previewHandoff.ts";

// --- fadeGainMultiplier ------------------------------------------------------

const fadeClip = { startS: 10, endS: 20, fadeInS: 2, fadeOutS: 4 };

// No fades → unity everywhere.
{
  const noFades = { ...fadeClip, fadeInS: null, fadeOutS: null };
  assert.equal(fadeGainMultiplier({ ...noFades, timelineTimeS: 10 }), 1);
  assert.equal(fadeGainMultiplier({ ...noFades, timelineTimeS: 19.99 }), 1);
}

// Linear ramp through the fade-in, unity in the middle.
{
  assert.equal(fadeGainMultiplier({ ...fadeClip, timelineTimeS: 10 }), 0);
  assert.equal(fadeGainMultiplier({ ...fadeClip, timelineTimeS: 11 }), 0.5);
  assert.equal(fadeGainMultiplier({ ...fadeClip, timelineTimeS: 12 }), 1);
  assert.equal(fadeGainMultiplier({ ...fadeClip, timelineTimeS: 14 }), 1);
}

// Linear ramp down through the fade-out.
{
  assert.equal(fadeGainMultiplier({ ...fadeClip, timelineTimeS: 16 }), 1);
  assert.equal(fadeGainMultiplier({ ...fadeClip, timelineTimeS: 18 }), 0.5);
  assert.equal(fadeGainMultiplier({ ...fadeClip, timelineTimeS: 20 }), 0);
}

// Overlapping fades on a short clip take the quieter ramp.
{
  const short = { startS: 0, endS: 2, fadeInS: 2, fadeOutS: 2 };
  assert.equal(fadeGainMultiplier({ ...short, timelineTimeS: 0.5 }), 0.25);
  assert.equal(fadeGainMultiplier({ ...short, timelineTimeS: 1.5 }), 0.25);
}

// Out-of-range times clamp to [0,1].
{
  assert.equal(fadeGainMultiplier({ ...fadeClip, timelineTimeS: 9 }), 0);
  assert.equal(fadeGainMultiplier({ ...fadeClip, timelineTimeS: 25 }), 0);
}

// --- isShuttleRate ---------------------------------------------------------

// Continuous (audible) decode up to and including the threshold;
// silent shuttle stepping above it.
{
  assert.equal(isShuttleRate(1), false);
  assert.equal(isShuttleRate(2.4), false); // common working speed — keeps audio
  assert.equal(isShuttleRate(SHUTTLE_MIN_RATE), false);
  assert.equal(isShuttleRate(SHUTTLE_MIN_RATE + 0.01), true);
  assert.equal(isShuttleRate(8), true);
  assert.equal(isShuttleRate(NaN), false);
}

// --- syncDriftThresholdS -------------------------------------------------

// Precise contexts (external seek, paused inspection) stay frame-accurate.
{
  assert.equal(
    syncDriftThresholdS({ precise: true, entry: false, elementPaused: false }),
    0.02,
  );
  // precise wins even when flagged as entry
  assert.equal(
    syncDriftThresholdS({ precise: true, entry: true, elementPaused: true }),
    0.02,
  );
}

// Segment entry while playing tolerates scheduling overshoot — the
// parked cut-in frame must NOT be re-seeked for sub-frame drift.
{
  const t = syncDriftThresholdS({
    precise: false,
    entry: true,
    elementPaused: true, // swapped-in slot is still paused at sync time
  });
  assert.equal(t, ENTRY_DRIFT_TOLERANCE_S);
  // covers worst-case rAF + effect latency at 2× speed (~0.1s)
  assert.ok(t >= 0.1);
}

// Steady-state playback only corrects runaway drift.
{
  assert.equal(
    syncDriftThresholdS({ precise: false, entry: false, elementPaused: false }),
    0.5,
  );
  assert.equal(
    syncDriftThresholdS({ precise: false, entry: false, elementPaused: true }),
    0.02,
  );
}

// --- driftRecoveryAction ---------------------------------------------------

const steady = { precise: false, entry: false, elementPaused: false };

// Within tolerance → leave everything alone.
{
  assert.equal(driftRecoveryAction({ ...steady, driftS: 0.1 }), "none");
  assert.equal(driftRecoveryAction({ ...steady, driftS: -0.3 }), "none");
  assert.equal(driftRecoveryAction({ ...steady, driftS: NaN }), "none");
}

// Element behind from decode underrun (seek latency, high-rate
// playback): the playhead follows the media closely — tight trigger
// so the cursor doesn't sawtooth ahead of lagging frames, and never
// a forward yank of the decoder.
{
  assert.equal(driftRecoveryAction({ ...steady, driftS: 0.2 }), "rebaseClock");
  assert.equal(driftRecoveryAction({ ...steady, driftS: 0.8 }), "rebaseClock");
  assert.equal(
    driftRecoveryAction({ ...steady, driftS: UNDERRUN_REBASE_MAX_S - 0.01 }),
    "rebaseClock",
  );
}

// Paused element during play-start latency: clock holds (rebase)
// from its own tight threshold instead of re-seeking the element.
{
  assert.equal(
    driftRecoveryAction({ ...steady, elementPaused: true, driftS: 0.05 }),
    "rebaseClock",
  );
}

// Hopeless lag or element AHEAD of the clock → real discontinuity.
{
  assert.equal(
    driftRecoveryAction({ ...steady, driftS: UNDERRUN_REBASE_MAX_S + 1 }),
    "seekElement",
  );
  assert.equal(driftRecoveryAction({ ...steady, driftS: -0.8 }), "seekElement");
}

// Precise contexts (external seek, paused stepping) always move the media.
{
  assert.equal(
    driftRecoveryAction({ precise: true, entry: false, elementPaused: false, driftS: 0.8 }),
    "seekElement",
  );
}

// Segment entry beyond tolerance (mis-parked preroll) seeks the media.
{
  assert.equal(
    driftRecoveryAction({ precise: false, entry: true, elementPaused: true, driftS: 0.8 }),
    "seekElement",
  );
  // …but a well-parked preroll's scheduling overshoot is left alone.
  assert.equal(
    driftRecoveryAction({ precise: false, entry: true, elementPaused: true, driftS: 0.05 }),
    "none",
  );
}

// --- timelineTimeForSegmentPosition ---------------------------------------

const seg = { timelineStart: 100, timelineEnd: 110, sourceStart: 50, speed: 1 };

// Plain mapping inside the segment.
{
  assert.equal(timelineTimeForSegmentPosition(seg, 53), 103);
}

// Speed scales source-seconds into timeline-seconds.
{
  assert.equal(
    timelineTimeForSegmentPosition({ ...seg, speed: 2 }, 54),
    102,
  );
}

// Positions outside the segment clamp to its range (float noise at
// edges must not escape into neighboring segments).
{
  assert.equal(timelineTimeForSegmentPosition(seg, 49), 100);
  assert.ok(timelineTimeForSegmentPosition(seg, 80) < 110);
  assert.ok(timelineTimeForSegmentPosition(seg, 80) >= 100);
}

// Garbage degrades to the segment start.
{
  assert.equal(timelineTimeForSegmentPosition(seg, NaN), 100);
  assert.equal(
    timelineTimeForSegmentPosition({ ...seg, speed: 0 }, 53),
    103,
  );
}

console.log("preview-handoff: all assertions passed");
