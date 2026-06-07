import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

from awidat_mcp import IndexAssetRequest

from composition_mcp import (
    _project_root_from,
    _region_for_shot,
    _regions_from_sidecars,
    _verify_regions,
)


class CompositionMcpTests(unittest.TestCase):
    def test_project_root_prefers_request_field_for_external_assets(self) -> None:
        with TemporaryDirectory() as project_dir, TemporaryDirectory() as media_dir:
            req = IndexAssetRequest(
                project_root=project_dir,
                asset_path=str(Path(media_dir) / "external.mp4"),
                asset_id="external/0001-external.mp4",
                asset_sha256="sha",
            )

            self.assertEqual(_project_root_from(req), Path(project_dir).absolute())

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

    def test_verify_regions_reports_invalid_ranges_and_confidence(self) -> None:
        report = _verify_regions(
            [
                {
                    "start_s": 0.0,
                    "end_s": 2.0,
                    "composition_confidence": 0.7,
                    "composition_source": "heuristic:composition-v1",
                },
                {
                    "start_s": 2.0,
                    "end_s": 1.5,
                    "composition_confidence": 1.4,
                    "composition_source": "",
                },
            ]
        )

        self.assertFalse(report["passed"])
        self.assertEqual(report["checked_regions"], 2)
        self.assertIn("region 1 has non-positive or non-finite range", report["issues"])
        self.assertIn("region 1 composition_confidence is outside 0..=1", report["issues"])
        self.assertIn("region 1 missing composition_source", report["issues"])


if __name__ == "__main__":
    unittest.main()
