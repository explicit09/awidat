#!/usr/bin/env python3
"""Tests for adaptive caption and overlay layout planning."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
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


class AdaptiveLayoutTests(unittest.TestCase):
    def test_moves_caption_away_from_face(self) -> None:
        adaptive_layout = load_script("adaptive_layout")
        phrase = {
            "id": "phrase-1",
            "text": "Watch this",
            "start_s": 1.0,
            "end_s": 2.0,
            "font_size": 52,
            "background": "rgba(0,0,0,0.64)",
            "stroke_width": 2,
            "safe_area": "mobile",
            "word_timings": [{"text": "Watch", "start_s": 1.0, "end_s": 1.3}],
        }
        face = {
            "detect_width": 1080,
            "detect_height": 1920,
            "per_frame": [{
                "t_s": 1.25,
                "faces": [{"box": [1280, 720, 1720, 360], "gaze_score": 0.02}],
            }],
        }

        plan = adaptive_layout.plan_adaptive_layout([phrase], face=face)

        recommendation = plan["recommendations"][0]
        self.assertEqual(recommendation["selected_zone"], "top")
        self.assertEqual(recommendation["edl_hint"]["position"], "top")
        self.assertEqual(recommendation["edl_hint"]["word_timings_json"], phrase["word_timings"])
        rejected = {item["zone"]: item["reasons"] for item in recommendation["rejected_zones"]}
        self.assertIn("overlaps_face", rejected["bottom"])
        self.assertGreater(recommendation["confidence"], 0.5)

    def test_keeps_scene_consistency_when_safe(self) -> None:
        adaptive_layout = load_script("adaptive_layout")
        phrases = [
            {"id": "p1", "text": "First idea", "start_s": 0.0, "end_s": 1.0},
            {"id": "p2", "text": "Second idea", "start_s": 1.1, "end_s": 2.0},
        ]
        face = {
            "detect_width": 1080,
            "detect_height": 1920,
            "per_frame": [
                {"t_s": 0.5, "faces": [{"box": [250, 700, 760, 320]}]},
                {"t_s": 1.5, "faces": [{"box": [260, 700, 770, 320]}]},
            ],
        }

        plan = adaptive_layout.plan_adaptive_layout(phrases, face=face)

        self.assertEqual(
            [item["selected_zone"] for item in plan["recommendations"]],
            ["bottom", "bottom"],
        )
        self.assertGreater(plan["recommendations"][1]["scores"]["bottom"]["consistency"], 0.9)

    def test_supports_overlay_items(self) -> None:
        adaptive_layout = load_script("adaptive_layout")
        overlay = {
            "id": "keyword-1",
            "overlay_kind": "keyword_emphasis",
            "text": "FAST",
            "start_s": 2.0,
            "end_s": 2.6,
            "font_size": 64,
            "background": "rgba(0,0,0,0.72)",
            "stroke_width": 2,
            "safe_area": "mobile",
        }

        plan = adaptive_layout.plan_adaptive_layout([overlay])

        recommendation = plan["recommendations"][0]
        self.assertEqual(recommendation["content_type"], "keyword_emphasis")
        self.assertEqual(recommendation["edl_hint"]["op"], "insert_title")
        self.assertIn(recommendation["style_recommendation"]["position"], {"top", "center", "bottom"})
        self.assertEqual(recommendation["edl_hint"]["layout_zone"], recommendation["selected_zone"])

    def test_uses_gaze_boxes_when_face_is_missing(self) -> None:
        adaptive_layout = load_script("adaptive_layout")
        gaze = {
            "detect_width": 1080,
            "detect_height": 1920,
            "per_frame": [
                {"t_s": 1.25, "faces": [{"box": [1300, 720, 1740, 360], "gaze_score": 0.2}]}
            ],
        }

        plan = adaptive_layout.plan_adaptive_layout(
            [{"text": "Look here", "start_s": 1.0, "end_s": 1.5}],
            gaze=gaze,
        )

        rejected = {item["zone"]: item["reasons"] for item in plan["recommendations"][0]["rejected_zones"]}
        self.assertIn("bottom", rejected)
        self.assertEqual(plan["recommendations"][0]["selected_zone"], "top")

    def test_rejects_key_action_regions(self) -> None:
        adaptive_layout = load_script("adaptive_layout")
        composition = {
            "regions": [{
                "start_s": 1.0,
                "end_s": 2.0,
                "composition_confidence": 0.9,
                "important_regions": [{
                    "kind": "key_action",
                    "box": {"x": 0.30, "y": 0.72, "width": 0.40, "height": 0.16},
                }],
            }]
        }

        plan = adaptive_layout.plan_adaptive_layout(
            [{"text": "Do not cover", "start_s": 1.0, "end_s": 2.0}],
            composition=composition,
        )

        rejected = {item["zone"]: item["reasons"] for item in plan["recommendations"][0]["rejected_zones"]}
        self.assertIn("overlaps_key_action", rejected["bottom"])
        self.assertNotEqual(plan["recommendations"][0]["selected_zone"], "bottom")

    def test_uses_frame_quality_for_readability_risk(self) -> None:
        adaptive_layout = load_script("adaptive_layout")
        frame_quality = {
            "per_frame": [
                {"t_s": 1.0, "blur": 80.0, "brightness": 22.0, "contrast": 15.0},
                {"t_s": 1.5, "blur": 70.0, "brightness": 230.0, "contrast": 12.0},
            ]
        }

        plan = adaptive_layout.plan_adaptive_layout(
            [{
                "text": "Needs box",
                "start_s": 1.0,
                "end_s": 2.0,
                "background": "transparent",
                "stroke_width": 0,
            }],
            frame_quality=frame_quality,
        )

        scores = plan["recommendations"][0]["scores"]["bottom"]
        self.assertLess(scores["readability"], 0.8)
        self.assertGreater(scores["motion_busy_region_risk"], 0.5)
        self.assertFalse(plan["recommendations"][0]["style_recommendation"]["boxed"])

    def test_resets_consistency_at_scene_boundary(self) -> None:
        adaptive_layout = load_script("adaptive_layout")
        phrases = [
            {"text": "First scene", "start_s": 0.0, "end_s": 1.0},
            {"text": "Second scene", "start_s": 3.0, "end_s": 4.0},
        ]
        composition = {
            "regions": [
                {"start_s": 0.0, "end_s": 2.0, "composition_confidence": 0.8},
                {"start_s": 2.0, "end_s": 5.0, "composition_confidence": 0.8},
            ]
        }

        plan = adaptive_layout.plan_adaptive_layout(phrases, composition=composition)

        self.assertEqual(plan["recommendations"][1]["scene_id"], "scene-2.000-5.000")
        self.assertEqual(plan["recommendations"][1]["scores"]["top"]["consistency"], 1.0)

    def test_caption_plan_cli_can_emit_adaptive_layout(self) -> None:
        transcript = {
            "words": [
                {"word": "Move", "start_s": 1.0, "end_s": 1.2, "speaker": "A"},
                {"word": "up", "start_s": 1.25, "end_s": 1.5, "speaker": "A"},
            ]
        }
        face = {
            "detect_width": 1080,
            "detect_height": 1920,
            "per_frame": [{"t_s": 1.25, "faces": [{"box": [1300, 720, 1740, 360]}]}],
        }
        with tempfile.NamedTemporaryFile("w", suffix=".json") as transcript_handle:
            json.dump(transcript, transcript_handle)
            transcript_handle.flush()
            with tempfile.NamedTemporaryFile("w", suffix=".json") as face_handle:
                json.dump({"data": face}, face_handle)
                face_handle.flush()
                result = subprocess.run(
                    [
                        sys.executable,
                        str(SCRIPT_DIR / "caption_plan.py"),
                        "--transcript",
                        transcript_handle.name,
                        "--face",
                        face_handle.name,
                        "--max-words",
                        "2",
                        "--output-format",
                        "adaptive-layout",
                    ],
                    check=True,
                    capture_output=True,
                    text=True,
                )

        payload = json.loads(result.stdout)
        self.assertEqual(payload["status"], "ready")
        self.assertEqual(payload["recommendations"][0]["selected_zone"], "top")

    def test_caption_plan_cli_can_include_overlay_layout_items(self) -> None:
        transcript = {"words": [{"word": "Base", "start_s": 1.0, "end_s": 1.2}]}
        overlays = {
            "items": [{
                "id": "topic-1",
                "overlay_kind": "topic_label",
                "text": "Launch plan",
                "start_s": 1.0,
                "end_s": 2.0,
            }]
        }
        with tempfile.NamedTemporaryFile("w", suffix=".json") as transcript_handle:
            json.dump(transcript, transcript_handle)
            transcript_handle.flush()
            with tempfile.NamedTemporaryFile("w", suffix=".json") as overlay_handle:
                json.dump(overlays, overlay_handle)
                overlay_handle.flush()
                result = subprocess.run(
                    [
                        sys.executable,
                        str(SCRIPT_DIR / "caption_plan.py"),
                        "--transcript",
                        transcript_handle.name,
                        "--layout-items",
                        overlay_handle.name,
                        "--output-format",
                        "adaptive-layout",
                    ],
                    check=True,
                    capture_output=True,
                    text=True,
                )

        recommendations = json.loads(result.stdout)["recommendations"]
        overlay_recommendation = next(item for item in recommendations if item["phrase_id"] == "topic-1")
        self.assertEqual(overlay_recommendation["content_type"], "topic_label")
        self.assertEqual(overlay_recommendation["edl_hint"]["op"], "insert_title")


if __name__ == "__main__":
    unittest.main()
