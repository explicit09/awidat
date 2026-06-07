import unittest

import base64
from pathlib import Path
from tempfile import TemporaryDirectory

import numpy as np
from montage_mcp import IndexAssetRequest

from shot_mcp import (
    _clip_match_candidates_for_shot,
    _composition_labels_for_shot,
    _composition_summary,
    _face_center_in_window,
    _gaze_summary_in_window,
    _project_root_from,
)


def _pack_embeddings(arr: np.ndarray) -> str:
    return base64.b64encode(arr.astype(np.float16).tobytes()).decode("ascii")


class ShotMcpTests(unittest.TestCase):
    def test_project_root_prefers_request_field_for_external_assets(self) -> None:
        with TemporaryDirectory() as project_dir, TemporaryDirectory() as media_dir:
            req = IndexAssetRequest(
                project_root=project_dir,
                asset_path=str(Path(media_dir) / "external.mp4"),
                asset_id="external/0001-external.mp4",
                asset_sha256="sha",
            )

            self.assertEqual(_project_root_from(req), Path(project_dir).absolute())

    def test_face_center_uses_area_weighted_largest_face_per_frame(self) -> None:
        per_frame = [
            {
                "t_s": 1.0,
                "faces": [
                    {"box": [10, 30, 30, 10]},
                    {"box": [40, 62, 58, 42]},
                ],
            },
            {
                "t_s": 2.0,
                "faces": [
                    {"box": [30, 70, 70, 30]},
                ],
            },
            {
                "t_s": 9.0,
                "faces": [
                    {"box": [0, 100, 100, 0]},
                ],
            },
        ]

        center = _face_center_in_window(per_frame, 100, 100, 0.0, 4.0)

        self.assertIsNotNone(center)
        assert center is not None
        self.assertAlmostEqual(center[0], 0.44)
        self.assertAlmostEqual(center[1], 0.44)

    def test_face_center_returns_none_without_usable_frame_dimensions(self) -> None:
        center = _face_center_in_window(
            [{"t_s": 1.0, "faces": [{"box": [10, 30, 30, 10]}]}],
            0,
            100,
            0.0,
            4.0,
        )

        self.assertIsNone(center)

    def test_gaze_summary_averages_largest_face_per_frame(self) -> None:
        per_frame = [
            {
                "t_s": 1.0,
                "faces": [
                    {"box": [0, 20, 20, 0], "gaze_score": 0.4, "at_camera": False},
                    {"box": [10, 50, 50, 10], "gaze_score": 0.08, "at_camera": True},
                ],
            },
            {
                "t_s": 2.0,
                "faces": [
                    {"box": [10, 30, 30, 10], "gaze_score": -0.22, "at_camera": False},
                ],
            },
            {
                "t_s": 9.0,
                "faces": [
                    {"box": [0, 100, 100, 0], "gaze_score": 0.0, "at_camera": True},
                ],
            },
        ]

        summary = _gaze_summary_in_window(per_frame, 0.0, 4.0)

        self.assertEqual(summary["samples"], 2)
        self.assertAlmostEqual(summary["gaze_score"], -0.07)
        self.assertAlmostEqual(summary["at_camera_ratio"], 0.5)
        self.assertEqual(summary["eye_trace"], "mixed")

    def test_gaze_summary_returns_none_when_no_faces_match_window(self) -> None:
        summary = _gaze_summary_in_window(
            [{"t_s": 9.0, "faces": [{"box": [0, 100, 100, 0], "gaze_score": 0.0}]}],
            0.0,
            4.0,
        )

        self.assertIsNone(summary)

    def test_composition_summary_derives_zone_headroom_and_look_space(self) -> None:
        summary = _composition_summary(
            subject_center=[0.24, 0.28],
            face_center=[0.24, 0.18],
            gaze_summary={"eye_trace": "right"},
        )

        self.assertEqual(summary["subject_zone"], "left_third")
        self.assertEqual(summary["face_zone"], "upper_left")
        self.assertEqual(summary["headroom"], "tight")
        self.assertEqual(summary["look_space"], "open")

    def test_composition_labels_for_shot_merges_overlapping_model_sidecar(self) -> None:
        labels = _composition_labels_for_shot(
            {
                "regions": [
                    {
                        "start_s": 8.0,
                        "end_s": 10.0,
                        "composition_source": "model:composition-v1",
                        "composition_confidence": 0.81,
                        "subject_role": "primary_speaker",
                        "depth_layer": "foreground",
                        "framing": "over_the_shoulder",
                    },
                    {
                        "start_s": 0.0,
                        "end_s": 2.0,
                        "composition_source": "model:composition-v1",
                        "composition_confidence": 0.94,
                        "subject_role": "detail_insert",
                        "depth_layer": "background",
                        "framing": "insert",
                    },
                ]
            },
            start_s=1.0,
            end_s=3.0,
        )

        self.assertEqual(labels["composition_source"], "model:composition-v1")
        self.assertAlmostEqual(labels["composition_confidence"], 0.94)
        self.assertEqual(labels["subject_role"], "detail_insert")
        self.assertEqual(labels["depth_layer"], "background")
        self.assertEqual(labels["framing"], "insert")

    def test_clip_match_candidates_exclude_same_shot_and_rank_similarity(self) -> None:
        clip_body = {
            "frame_count": 4,
            "embedding_dim": 3,
            "timestamps_s": [1.0, 3.0, 8.0, 12.0],
            "embeddings_b64": _pack_embeddings(
                np.array(
                    [
                        [1.0, 0.0, 0.0],
                        [1.0, 0.0, 0.0],
                        [0.98, 0.02, 0.0],
                        [0.0, 1.0, 0.0],
                    ],
                    dtype=np.float32,
                )
            ),
        }

        candidates = _clip_match_candidates_for_shot(
            clip_body,
            start_s=0.0,
            end_s=4.0,
            limit=2,
        )

        self.assertEqual(len(candidates), 2)
        self.assertAlmostEqual(candidates[0]["time_s"], 8.0)
        self.assertGreater(candidates[0]["similarity"], 0.97)
        self.assertEqual(candidates[0]["basis"], "clip_embedding")
        self.assertGreater(candidates[0]["confidence"], candidates[1]["confidence"])
        self.assertAlmostEqual(candidates[1]["time_s"], 12.0)

    def test_clip_match_candidates_can_rank_across_assets(self) -> None:
        source_body = {
            "frame_count": 2,
            "embedding_dim": 3,
            "timestamps_s": [1.0, 3.0],
            "embeddings_b64": _pack_embeddings(
                np.array([[1.0, 0.0, 0.0], [1.0, 0.0, 0.0]], dtype=np.float32)
            ),
        }
        other_body = {
            "frame_count": 2,
            "embedding_dim": 3,
            "timestamps_s": [4.0, 9.0],
            "embeddings_b64": _pack_embeddings(
                np.array([[0.99, 0.01, 0.0], [0.0, 1.0, 0.0]], dtype=np.float32)
            ),
        }

        candidates = _clip_match_candidates_for_shot(
            source_body,
            start_s=0.0,
            end_s=4.0,
            limit=2,
            asset_id="raw/a.mp4",
            candidate_clip_bodies=[
                ("raw/a.mp4", source_body),
                ("raw/b.mp4", other_body),
            ],
        )

        self.assertEqual(candidates[0]["asset_id"], "raw/b.mp4")
        self.assertAlmostEqual(candidates[0]["time_s"], 4.0)
        self.assertGreater(candidates[0]["similarity"], 0.98)

    def test_clip_match_candidates_deduplicate_adjacent_frames_before_limit(self) -> None:
        source_body = {
            "frame_count": 1,
            "embedding_dim": 3,
            "timestamps_s": [1.0],
            "embeddings_b64": _pack_embeddings(
                np.array([[1.0, 0.0, 0.0]], dtype=np.float32)
            ),
        }
        crowded_body = {
            "frame_count": 3,
            "embedding_dim": 3,
            "timestamps_s": [5.0, 5.2, 11.0],
            "embeddings_b64": _pack_embeddings(
                np.array(
                    [
                        [0.999, 0.001, 0.0],
                        [0.997, 0.003, 0.0],
                        [0.93, 0.07, 0.0],
                    ],
                    dtype=np.float32,
                )
            ),
        }

        candidates = _clip_match_candidates_for_shot(
            source_body,
            start_s=0.0,
            end_s=2.0,
            limit=2,
            asset_id="raw/source.mp4",
            candidate_clip_bodies=[
                ("raw/source.mp4", source_body),
                ("raw/crowded.mp4", crowded_body),
            ],
        )

        self.assertEqual(len(candidates), 2)
        self.assertAlmostEqual(candidates[0]["time_s"], 5.0)
        self.assertAlmostEqual(candidates[1]["time_s"], 11.0)


if __name__ == "__main__":
    unittest.main()
