#!/usr/bin/env python3
"""Tests for beat cut planning helpers."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("beat_cut_plan.py")
SPEC = importlib.util.spec_from_file_location("beat_cut_plan", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
beat_cut_plan = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(beat_cut_plan)


class BeatCutPlanTest(unittest.TestCase):
    def test_fixed_cadence_without_energy(self) -> None:
        cuts = beat_cut_plan.plan_cuts(
            beats=[0.0, 0.5, 1.0, 1.5, 2.0],
            cut_every=2,
            duration_s=None,
            shot={},
            audio_energy={},
        )
        self.assertEqual([c["target_s"] for c in cuts], [0.0, 1.0, 2.0])
        self.assertTrue(all(c["selection_reason"] == "cadence" for c in cuts))

    def test_energy_mode_prefers_loud_accent_beats(self) -> None:
        energy = {
            "windows": [
                {"start_s": 0.0, "rms_db": -45.0},
                {"start_s": 0.5, "rms_db": -18.0},
                {"start_s": 1.0, "rms_db": -44.0},
                {"start_s": 1.5, "rms_db": -16.0},
                {"start_s": 2.0, "rms_db": -42.0},
                {"start_s": 2.5, "rms_db": -15.0},
            ]
        }
        cuts = beat_cut_plan.plan_cuts(
            beats=[0.0, 0.5, 1.0, 1.5, 2.0, 2.5],
            cut_every=2,
            duration_s=None,
            shot={},
            audio_energy=energy,
        )
        self.assertEqual([c["target_s"] for c in cuts], [0.5, 1.5, 2.5])
        self.assertTrue(all(c["selection_reason"] == "energy_peak" for c in cuts))
        self.assertGreater(cuts[0]["energy_score"], cuts[0]["quiet_floor_db"])

    def test_motion_context_is_preserved_in_energy_mode(self) -> None:
        shot = {"shots": [{"start_s": 0.4, "end_s": 0.6, "motion": "fast-cut"}]}
        energy = {"windows": [{"start_s": 0.5, "rms_db": -18.0}]}
        cuts = beat_cut_plan.plan_cuts(
            beats=[0.0, 0.5],
            cut_every=1,
            duration_s=None,
            shot=shot,
            audio_energy=energy,
        )
        self.assertTrue(cuts[1]["near_motion"])


if __name__ == "__main__":
    unittest.main()
