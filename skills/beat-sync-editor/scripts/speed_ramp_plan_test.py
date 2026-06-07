#!/usr/bin/env python3
"""Tests for beat-synced speed-ramp planning helpers."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("speed_ramp_plan.py")
SPEC = importlib.util.spec_from_file_location("speed_ramp_plan", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
speed_ramp_plan = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(speed_ramp_plan)


class SpeedRampPlanTest(unittest.TestCase):
    def test_plans_time_remap_curve_around_accented_beats(self) -> None:
        plan = speed_ramp_plan.plan_speed_ramps(
            beats=[0.0, 0.5, 1.0, 1.5, 2.0],
            duration_s=2.0,
            accent_every=2,
            slow_factor=0.5,
            fast_factor=1.25,
            ramp_window_s=0.2,
        )

        self.assertEqual(plan["effect"], "montage.time_remap")
        self.assertEqual(plan["accent_beats"], [0, 2, 4])
        self.assertEqual(plan["nominal_speed"], 1.0)
        self.assertEqual(plan["speed_points"][0], {"time_s": 0.0, "factor": 0.5})
        self.assertIn({"time_s": 0.2, "factor": 1.25}, plan["speed_points"])
        self.assertIn({"time_s": 1.0, "factor": 0.5}, plan["speed_points"])
        self.assertEqual(plan["curve"][0], {"source_time_s": 0.0, "timeline_time_s": 0.0})
        self.assertAlmostEqual(plan["curve"][-1]["source_time_s"], 2.0)
        self.assertGreater(plan["curve"][-1]["timeline_time_s"], 0.0)

    def test_filters_beats_outside_duration_and_requires_positive_duration(self) -> None:
        plan = speed_ramp_plan.plan_speed_ramps(
            beats=[0.0, 0.5, 1.0, 4.0],
            duration_s=1.0,
            accent_every=2,
            slow_factor=0.4,
            fast_factor=1.3,
            ramp_window_s=0.15,
        )

        self.assertEqual(plan["accent_beats"], [0, 2])
        self.assertEqual(plan["curve"][-1]["source_time_s"], 1.0)

        with self.assertRaisesRegex(ValueError, "duration_s must be > 0"):
            speed_ramp_plan.plan_speed_ramps(
                beats=[0.0],
                duration_s=0.0,
                accent_every=2,
                slow_factor=0.4,
                fast_factor=1.3,
                ramp_window_s=0.15,
            )

    def test_formats_set_effect_edl_for_time_remap_curve(self) -> None:
        plan = speed_ramp_plan.plan_speed_ramps(
            beats=[0.0, 0.5, 1.0],
            duration_s=1.0,
            accent_every=2,
            slow_factor=0.5,
            fast_factor=1.25,
            ramp_window_s=0.2,
        )

        edl = speed_ramp_plan.format_set_effect_edl(
            anchor="clip:hero-shot",
            plan=plan,
            rationale="hit downbeats with speed accents",
        )

        self.assertIn("*** Set Effect", edl)
        self.assertIn("@@ anchor: clip:hero-shot", edl)
        self.assertIn("+ effect: montage.time_remap", edl)
        self.assertIn("+ rationale: hit downbeats with speed accents", edl)
        self.assertIn('"curve"', edl)
        self.assertIn('"source_time_s": 1.0', edl)


if __name__ == "__main__":
    unittest.main()
