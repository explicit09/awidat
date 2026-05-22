import unittest

from frame_quality_mcp import _thumbnail_candidates, _thumbnail_score


class FrameQualityThumbnailTests(unittest.TestCase):
    def test_thumbnail_score_prefers_sharp_balanced_contrast_frames(self) -> None:
        strong = {
            "blur": 240.0,
            "brightness": 128.0,
            "contrast": 72.0,
            "is_sharp": True,
        }
        weak = {
            "blur": 35.0,
            "brightness": 18.0,
            "contrast": 8.0,
            "is_sharp": False,
        }

        self.assertGreater(_thumbnail_score(strong), 0.80)
        self.assertLess(_thumbnail_score(weak), 0.30)

    def test_thumbnail_candidates_rank_and_explain_top_frames(self) -> None:
        per_frame = [
            {
                "t_s": 0.0,
                "blur": 20.0,
                "brightness": 12.0,
                "contrast": 6.0,
                "is_sharp": False,
            },
            {
                "t_s": 1.0,
                "blur": 260.0,
                "brightness": 130.0,
                "contrast": 80.0,
                "is_sharp": True,
            },
            {
                "t_s": 2.0,
                "blur": 120.0,
                "brightness": 245.0,
                "contrast": 18.0,
                "is_sharp": True,
            },
        ]

        candidates = _thumbnail_candidates(per_frame, limit=2)

        self.assertEqual([candidate["t_s"] for candidate in candidates], [1.0, 2.0])
        self.assertGreater(candidates[0]["thumbnail_score"], candidates[1]["thumbnail_score"])
        self.assertIn("sharp", candidates[0]["reasons"])
        self.assertIn("balanced_exposure", candidates[0]["reasons"])


if __name__ == "__main__":
    unittest.main()
