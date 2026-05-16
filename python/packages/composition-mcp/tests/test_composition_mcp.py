import unittest

from composition_mcp import _region_for_shot, _regions_from_sidecars


class CompositionMcpTests(unittest.TestCase):
    def test_region_for_shot_labels_primary_speaker_foreground(self) -> None:
        region = _region_for_shot(
            {"start_s": 0.0, "end_s": 4.0},
            {
                "detect_width": 100,
                "detect_height": 100,
                "per_frame": [
                    {"t_s": 1.0, "faces": [{"box": [10, 70, 70, 10]}]},
                    {"t_s": 2.0, "faces": [{"box": [12, 72, 72, 12]}]},
                ],
            },
            {
                "per_frame": [
                    {
                        "t_s": 1.0,
                        "faces": [{"box": [10, 70, 70, 10], "gaze_score": 0.04}],
                    }
                ]
            },
        )

        self.assertEqual(region["composition_source"], "heuristic:composition-v1")
        self.assertGreater(region["composition_confidence"], 0.80)
        self.assertEqual(region["subject_role"], "primary_speaker")
        self.assertEqual(region["depth_layer"], "foreground")
        self.assertEqual(region["framing"], "extreme_close_up")

    def test_regions_from_sidecars_falls_back_to_duration_without_shots(self) -> None:
        regions = _regions_from_sidecars(
            {"duration_s": 6.5},
            None,
            None,
            None,
        )

        self.assertEqual(len(regions), 1)
        self.assertEqual(regions[0]["start_s"], 0.0)
        self.assertEqual(regions[0]["end_s"], 6.5)
        self.assertEqual(regions[0]["subject_role"], "environment")
        self.assertEqual(regions[0]["framing"], "wide_context")

    def test_regions_from_sidecars_prefers_overlapping_model_region(self) -> None:
        regions = _regions_from_sidecars(
            {"shots": [{"start_s": 10.0, "end_s": 14.0}]},
            {
                "detect_width": 100,
                "detect_height": 100,
                "per_frame": [{"t_s": 11.0, "faces": [{"box": [20, 55, 55, 20]}]}],
            },
            None,
            {
                "regions": [
                    {
                        "start_s": 9.5,
                        "end_s": 13.5,
                        "composition_source": "model:composition-v2",
                        "composition_confidence": 0.94,
                        "subject_role": "primary_speaker",
                        "depth_layer": "foreground",
                        "framing": "single_close",
                    }
                ]
            },
        )

        self.assertEqual(regions[0]["composition_source"], "model:composition-v2")
        self.assertEqual(regions[0]["composition_confidence"], 0.94)
        self.assertEqual(regions[0]["subject_role"], "primary_speaker")
        self.assertEqual(regions[0]["depth_layer"], "foreground")
        self.assertEqual(regions[0]["framing"], "single_close")
        self.assertIn("heuristic_composition_source", regions[0])


if __name__ == "__main__":
    unittest.main()
