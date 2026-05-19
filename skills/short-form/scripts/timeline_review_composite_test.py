#!/usr/bin/env python3
"""Tests for agent-readable timeline review composites."""

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent


def load_script(name: str):
    spec = importlib.util.spec_from_file_location(name, SCRIPT_DIR / f"{name}.py")
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load {name}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class TimelineReviewCompositeTests(unittest.TestCase):
    def test_build_composite_plan_maps_words_silences_and_cut_points(self) -> None:
        composite = load_script("timeline_review_composite")

        plan = composite.build_composite_plan(
            {
                "duration_s": 6.0,
                "words": [
                    {"start_s": 0.5, "end_s": 1.0, "text": "Hook"},
                    {"start_s": 2.0, "end_s": 2.6, "text": "beat"},
                ],
                "silences": [{"start_s": 3.0, "end_s": 4.0}],
                "cut_points_s": [1.5, 4.5],
            },
            width=600,
            height=320,
        )

        self.assertEqual(plan["status"], "ready")
        self.assertEqual(plan["word_count"], 2)
        self.assertEqual(plan["silence_count"], 1)
        self.assertEqual(plan["cut_count"], 2)
        self.assertGreater(plan["layers"]["words"][0]["x"], 0)
        self.assertLess(plan["layers"]["words"][0]["x"], plan["layers"]["words"][1]["x"])
        self.assertGreater(plan["layers"]["silences"][0]["width"], 20)
        self.assertEqual(plan["layers"]["cuts"][0]["x"], 150)

    def test_write_review_composite_outputs_png_and_json_evidence(self) -> None:
        composite = load_script("timeline_review_composite")

        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "timeline-review.png"
            report = composite.write_review_composite(
                {
                    "duration_s": 5.0,
                    "source_label": "source",
                    "output_label": "output",
                    "words": [
                        {"start_s": 0.25, "end_s": 0.8, "text": "sharp"},
                        {"start_s": 1.0, "end_s": 1.4, "text": "cut"},
                    ],
                    "silences": [{"start_s": 2.0, "end_s": 2.8}],
                    "cut_points_s": [1.5],
                },
                output_path=out,
            )

            sidecar = out.with_suffix(".json")
            self.assertTrue(out.exists())
            self.assertEqual(out.read_bytes()[:8], b"\x89PNG\r\n\x1a\n")
            self.assertTrue(sidecar.exists())
            sidecar_json = json.loads(sidecar.read_text())

        self.assertEqual(report["status"], "ready")
        self.assertEqual(report["artifact"], str(out))
        self.assertEqual(sidecar_json["artifact"], str(out))
        self.assertEqual(sidecar_json["word_count"], 2)
        self.assertEqual(sidecar_json["silence_count"], 1)
        self.assertIn("inspect generated timeline review composite", report["message"])


if __name__ == "__main__":
    unittest.main()
