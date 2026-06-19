#!/usr/bin/env python3
"""Tests for dynamic short-form layout planning."""

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))


def load_script(name: str):
    spec = importlib.util.spec_from_file_location(name, SCRIPT_DIR / f"{name}.py")
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load {name}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ShortFormLayoutPlanTests(unittest.TestCase):
    def test_extended_turn_becomes_active_speaker_fill(self) -> None:
        planner = load_script("short_form_layout_plan")
        segments = [
            {"start_s": 10.0, "end_s": 14.0, "speaker": "Speaker 0"},
            {"start_s": 15.0, "end_s": 24.0, "speaker": "Speaker 0"},
        ]

        plan = planner.plan_short_form_layout(
            planner.normalize_speaker_segments({"segments": segments}),
            clip_start_s=10.0,
            clip_end_s=24.0,
            speaker_slot_evidence={"0": {"slot": "left", "confidence": 0.9}},
            min_fill_s=8.0,
        )

        self.assertEqual(plan["status"], "ready")
        self.assertEqual(plan["layouts"][0]["mode"], "fill")
        self.assertEqual(plan["layouts"][0]["active_speaker"], "0")
        self.assertEqual(plan["layouts"][0]["active_slot"], "left")

    def test_short_dialogue_places_active_speaker_on_top(self) -> None:
        planner = load_script("short_form_layout_plan")
        segments = [
            {"start_s": 0.0, "end_s": 2.0, "speaker": "Speaker 0"},
            {"start_s": 2.2, "end_s": 5.0, "speaker": "Speaker 1"},
            {"start_s": 5.2, "end_s": 7.0, "speaker": "Speaker 0"},
        ]

        plan = planner.plan_short_form_layout(
            planner.normalize_speaker_segments({"segments": segments}),
            clip_start_s=0.0,
            clip_end_s=7.0,
            speaker_to_slot={"0": "left", "1": "right"},
            min_fill_s=8.0,
            min_layout_s=1.0,
        )

        self.assertEqual(
            [(item["mode"], item.get("top_speaker"), item.get("bottom_speaker")) for item in plan["layouts"]],
            [
                ("split_stacked", "0", "1"),
                ("split_stacked", "1", "0"),
                ("split_stacked", "0", "1"),
            ],
        )
        self.assertEqual(plan["layouts"][1]["top_slot"], "right")
        self.assertEqual(plan["layouts"][1]["bottom_slot"], "left")

    def test_dominant_speaker_uses_fill_for_whole_clip(self) -> None:
        planner = load_script("short_form_layout_plan")
        segments = [
            {"start_s": 0.0, "end_s": 16.0, "speaker": "Speaker 0"},
            {"start_s": 16.2, "end_s": 18.0, "speaker": "Speaker 1"},
        ]

        plan = planner.plan_short_form_layout(
            planner.normalize_speaker_segments({"segments": segments}),
            clip_start_s=0.0,
            clip_end_s=18.0,
            speaker_slot_evidence={
                "0": {"slot": "left", "confidence": 0.9, "method": "lip_activity"},
                "1": {"slot": "right", "confidence": 0.9, "method": "lip_activity"},
            },
        )

        self.assertEqual(plan["status"], "ready")
        self.assertEqual(len(plan["layouts"]), 1)
        self.assertEqual(plan["layouts"][0]["mode"], "fill")
        self.assertEqual(plan["layouts"][0]["active_speaker"], "0")
        self.assertEqual(plan["layouts"][0]["active_slot"], "left")

    def test_low_confidence_slot_mapping_requires_review(self) -> None:
        planner = load_script("short_form_layout_plan")
        segments = [
            {"start_s": 0.0, "end_s": 4.0, "speaker": "Speaker 0"},
            {"start_s": 4.2, "end_s": 8.0, "speaker": "Speaker 1"},
        ]

        plan = planner.plan_short_form_layout(
            planner.normalize_speaker_segments({"segments": segments}),
            clip_start_s=0.0,
            clip_end_s=8.0,
            speaker_to_slot={"0": "left"},
            min_layout_s=1.0,
        )

        self.assertEqual(plan["status"], "needs_review")
        self.assertIn("speaker_slot_mapping_needs_visual_verification", plan["warnings"])


if __name__ == "__main__":
    unittest.main()
