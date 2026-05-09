import unittest

import numpy as np

from color_analysis_mcp import _scenes, _score_frame, _summarize


class ColorAnalysisTests(unittest.TestCase):
    def test_dark_frame_recommends_positive_exposure(self) -> None:
        frame = np.full((64, 64, 3), 5, dtype=np.uint8)
        scored = _score_frame(frame, 0.0)
        self.assertGreater(scored["recommended_correction"]["exposure_ev"], 0.5)
        self.assertGreater(scored["underexposed_fraction"], 0.0)
        self.assertIn("underexposed", scored["issue_tags"])

    def test_bright_frame_recommends_negative_highlights(self) -> None:
        frame = np.full((64, 64, 3), 250, dtype=np.uint8)
        scored = _score_frame(frame, 0.0)
        self.assertLess(scored["recommended_correction"]["exposure_ev"], -0.5)
        self.assertLess(scored["recommended_correction"]["highlights"], 0.0)
        self.assertIn("overexposed", scored["issue_tags"])

    def test_cool_cast_recommends_warming_temperature(self) -> None:
        frame = np.zeros((64, 64, 3), dtype=np.uint8)
        frame[:, :, 0] = 180  # B channel
        frame[:, :, 1] = 110
        frame[:, :, 2] = 80
        scored = _score_frame(frame, 0.0)
        self.assertEqual(scored["estimated_color_cast"], "cool")
        self.assertGreater(scored["recommended_correction"]["temperature"], 0.0)
        self.assertIn("cool_cast", scored["issue_tags"])

    def test_balanced_gradient_is_not_auto_corrected(self) -> None:
        row = np.linspace(45, 210, 64, dtype=np.uint8)
        gray = np.tile(row, (64, 1))
        frame = np.dstack([gray, gray, gray])
        scored = _score_frame(frame, 0.0)
        self.assertIn("already_balanced", scored["issue_tags"])
        self.assertFalse(scored["auto_correct_safe"])
        self.assertLess(abs(scored["recommended_correction"]["exposure_ev"]), 0.25)

    def test_summary_preserves_recommendation_shape(self) -> None:
        frames = [
            _score_frame(np.full((32, 32, 3), 90, dtype=np.uint8), 0.0),
            _score_frame(np.full((32, 32, 3), 130, dtype=np.uint8), 1.0),
        ]
        summary = _summarize(frames)
        self.assertIn("recommended_correction", summary)
        self.assertIn("brightness_mean", summary)
        self.assertIn("estimated_color_cast", summary)
        self.assertIn("confidence", summary)
        self.assertIn("issue_tags", summary)
        self.assertIn("auto_correct_safe", summary)
        self.assertIn("policy", summary)
        self.assertIn("recommended_action", summary["policy"])

    def test_scene_segmentation_groups_distinct_issue_signatures(self) -> None:
        frames = [
            _score_frame(np.full((32, 32, 3), 55, dtype=np.uint8), 0.0),
            _score_frame(np.full((32, 32, 3), 58, dtype=np.uint8), 1.0),
            _score_frame(np.full((32, 32, 3), 205, dtype=np.uint8), 2.0),
            _score_frame(np.full((32, 32, 3), 208, dtype=np.uint8), 3.0),
        ]
        scenes = _scenes(frames)
        self.assertGreaterEqual(len(scenes), 2)
        self.assertIn("recommended_correction", scenes[0])
        self.assertIn("confidence", scenes[0])
        self.assertIn("policy", scenes[0])

    def test_summary_ignores_black_frames_when_usable_frames_exist(self) -> None:
        frames = [
            _score_frame(np.full((32, 32, 3), 0, dtype=np.uint8), 0.0),
            _score_frame(np.full((32, 32, 3), 120, dtype=np.uint8), 1.0),
        ]
        summary = _summarize(frames)
        self.assertGreater(summary["brightness_mean"], 100.0)

    def test_policy_routes_balanced_auto_and_review_cases(self) -> None:
        balanced = _summarize([
            _score_frame(np.dstack([np.tile(np.linspace(45, 210, 64, dtype=np.uint8), (64, 1))] * 3), 0.0)
        ])
        self.assertEqual(balanced["policy"]["recommended_action"], "no_op")
        self.assertFalse(balanced["policy"]["graph_ops"])

        dark = _summarize([
            _score_frame(np.full((64, 64, 3), 60, dtype=np.uint8), 0.0),
            _score_frame(np.full((64, 64, 3), 70, dtype=np.uint8), 1.0),
        ])
        self.assertEqual(dark["policy"]["recommended_action"], "auto_correct")
        self.assertIn("lighting_correction", dark["policy"]["edit_types"])
        self.assertEqual(dark["policy"]["graph_ops"], ["Set Color Correction"])

        crushed = _summarize([
            _score_frame(np.full((64, 64, 3), 0, dtype=np.uint8), 0.0),
            _score_frame(np.full((64, 64, 3), 5, dtype=np.uint8), 1.0),
        ])
        self.assertEqual(crushed["policy"]["recommended_action"], "review_only")
        self.assertTrue(crushed["policy"]["requires_contact_sheet"])


if __name__ == "__main__":
    unittest.main()
