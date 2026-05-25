"""Tests for the talking-head vertical planning helper."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("talking_head_plan.py")


def load_script():
    spec = importlib.util.spec_from_file_location("talking_head_plan", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def face_frame(t_s: float, *, x: float = 0.58, y: float = 0.12, w: float = 0.22, h: float = 0.32):
    return {
        "t_s": t_s,
        "faces": [
            {
                "bbox": {"x": x, "y": y, "w": w, "h": h},
                "confidence": 0.94,
            }
        ],
    }


def word(text: str, start: float, end: float):
    return {"word": text, "start_s": start, "end_s": end, "speaker": "A"}


def fixture_inputs():
    transcript = {
        "segments": [
            {
                "start_s": 0.0,
                "end_s": 2.4,
                "text": "Here is the mistake founders keep making",
            },
            {
                "start_s": 2.4,
                "end_s": 6.0,
                "text": "they wait until everything feels perfect",
            },
        ],
        "words": [
            word("Here", 0.0, 0.28),
            word("is", 0.3, 0.48),
            word("the", 0.5, 0.66),
            word("mistake", 0.7, 1.05),
            word("founders", 1.1, 1.48),
            word("keep", 1.52, 1.8),
            word("making", 1.84, 2.2),
            word("they", 2.45, 2.7),
            word("wait", 2.75, 3.08),
            word("until", 3.1, 3.42),
            word("everything", 3.5, 3.96),
            word("feels", 4.0, 4.28),
            word("perfect", 4.32, 4.72),
        ],
    }
    face = {"per_frame": [face_frame(t) for t in [0, 1, 2, 3, 4, 5]]}
    gaze = {
        "per_frame": [
            {"t_s": t, "faces": [{"at_camera": True}]} for t in [0, 1, 2, 3, 4, 5]
        ]
    }
    shot = {
        "shots": [
            {
                "start_s": 0,
                "end_s": 6,
                "type": "medium",
                "motion": "locked",
            }
        ]
    }
    composition = {
        "regions": [
            {
                "start_s": 0,
                "end_s": 6,
                "negative_space": {"side": "left", "score": 0.72},
                "framing": "good",
            }
        ]
    }
    quality = {"per_frame": [{"t_s": t, "is_sharp": True} for t in [0, 1, 2, 3, 4, 5]]}
    energy = {
        "windows": [{"start_s": t, "rms_db": -24} for t in [0, 1, 2, 3, 4, 5]],
        "silences": [{"start_s": 4.8, "end_s": 5.8}],
    }
    moments = {
        "moments": [
            {
                "id": "hook-1",
                "kind": "hook",
                "score": 0.92,
                "start_s": 0.0,
                "end_s": 2.4,
                "note": "mistake founders keep making",
            }
        ]
    }
    topic = {"topics": [{"start_s": 0.0, "end_s": 10.0, "label": "founder advice"}]}
    return {
        "transcript": transcript,
        "face": face,
        "gaze": gaze,
        "shot": shot,
        "composition": composition,
        "frame_quality": quality,
        "audio_energy": energy,
        "moments": moments,
        "topic": topic,
    }


class TalkingHeadPlanTest(unittest.TestCase):
    def test_candidate_layout_and_edl_use_talking_head_signals(self):
        planner = load_script()
        plan = planner.build_talking_head_plan(
            fixture_inputs(),
            asset_id="clip.mov",
            clip_id="clip-1",
            source_width=1920,
            source_height=1080,
        )

        self.assertTrue(plan["candidate"]["is_candidate"])
        self.assertEqual(plan["layout"]["strategy"], "reframe_horizontal_to_9_16")
        self.assertEqual(plan["layout"]["caption_position"], "bottom")
        self.assertEqual(plan["layout"]["reserved_space"]["side"], "left")
        self.assertEqual(plan["hook"]["start_s"], 0.0)
        self.assertEqual(plan["readiness"]["status"], "ready_for_review")
        self.assertIn("Set Output Format", plan["edl"])
        self.assertIn("@@ anchor: clip_uuid=clip-1", plan["edl"])
        self.assertIn("awidat.reframe", plan["edl"])
        self.assertIn("Split Clip", plan["edl"])
        self.assertIn("Insert Caption", plan["edl"])
        self.assertIn("Set Parameter Animation", plan["edl"])

    def test_native_vertical_is_kept_when_framing_is_good(self):
        planner = load_script()
        plan = planner.build_talking_head_plan(
            fixture_inputs(),
            asset_id="clip.mov",
            clip_id="clip-1",
            source_width=1080,
            source_height=1920,
        )

        self.assertEqual(plan["layout"]["strategy"], "keep_native_vertical")
        self.assertNotIn("awidat.reframe", plan["edl"])

    def test_face_overlap_moves_captions_to_safe_empty_space(self):
        planner = load_script()
        inputs = fixture_inputs()
        inputs["face"]["per_frame"] = [
            face_frame(t, x=0.39, y=0.54, w=0.24, h=0.34) for t in [0, 1, 2]
        ]
        inputs["composition"]["regions"][0]["negative_space"] = {"side": "top", "score": 0.8}

        plan = planner.build_talking_head_plan(
            inputs,
            asset_id="clip.mov",
            clip_id="clip-1",
            source_width=1080,
            source_height=1920,
        )

        self.assertEqual(plan["layout"]["caption_position"], "top")
        self.assertEqual(plan["caption_plan"]["face_overlap_risk"], "avoided")

    def test_non_talking_head_candidate_is_blocked(self):
        planner = load_script()
        inputs = fixture_inputs()
        inputs["face"] = {"per_frame": []}
        inputs["gaze"] = {"per_frame": []}

        plan = planner.build_talking_head_plan(
            inputs,
            asset_id="clip.mov",
            clip_id="clip-1",
            source_width=1920,
            source_height=1080,
        )

        self.assertFalse(plan["candidate"]["is_candidate"])
        self.assertEqual(plan["readiness"]["status"], "blocked")
        self.assertIn("no sustained face evidence", plan["readiness"]["issues"])

    def test_low_value_repetition_is_flagged_for_pacing_review(self):
        planner = load_script()
        inputs = fixture_inputs()
        inputs["transcript"]["segments"] = [
            {"start_s": 0.0, "end_s": 2.0, "text": "The launch failed because support was manual"},
            {"start_s": 2.1, "end_s": 4.2, "text": "The launch failed because support was manual"},
        ]

        plan = planner.build_talking_head_plan(
            inputs,
            asset_id="clip.mov",
            clip_id="clip-1",
            source_width=1080,
            source_height=1920,
        )

        self.assertTrue(
            any(
                cluster["reason"] == "low_value_repetition_review"
                for cluster in plan["pacing_plan"]["filler_clusters"]
            )
        )


if __name__ == "__main__":
    unittest.main()
