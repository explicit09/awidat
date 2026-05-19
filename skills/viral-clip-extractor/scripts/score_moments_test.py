#!/usr/bin/env python3
"""Tests for the viral moment scorecard helper."""

from __future__ import annotations

import unittest

import score_moments


class ScoreMomentsTest(unittest.TestCase):
    def test_score_candidates_exposes_component_scorecard(self) -> None:
        candidates = score_moments.score_candidates(
            moments_body={
                "moments": [
                    {
                        "id": "m1",
                        "kind": "hook",
                        "start_s": 10.0,
                        "end_s": 42.0,
                        "score": 0.8,
                        "note": "the real problem",
                    }
                ]
            },
            energy_body={
                "windows": [
                    {"start_s": 12.0, "rms_db": -18.0},
                    {"start_s": 30.0, "rms_db": -16.0},
                ]
            },
            transcript_body={
                "segments": [
                    {
                        "start_s": 10.0,
                        "end_s": 42.0,
                        "text": "Here is the secret problem nobody explains.",
                    }
                ]
            },
            shot_body={
                "shots": [
                    {
                        "start_s": 9.0,
                        "end_s": 43.0,
                        "type": "close-up",
                        "motion": "handheld",
                    }
                ]
            },
            gaze_body={
                "per_frame": [
                    {"t_s": 12.0, "faces": [{"at_camera": True}]},
                    {"t_s": 13.0, "faces": [{"at_camera": False}]},
                    {"t_s": 14.0, "faces": [{"at_camera": True}]},
                ]
            },
            quality_body={
                "per_frame": [
                    {"t_s": 12.0, "is_sharp": True},
                    {"t_s": 13.0, "is_sharp": True},
                    {"t_s": 14.0, "is_sharp": True},
                ]
            },
            topic_body={"topics": [{"start_s": 8.0, "end_s": 44.0}]},
            limit=1,
        )

        self.assertEqual(len(candidates), 1)
        candidate = candidates[0]
        self.assertEqual(candidate["moment_id"], "m1")
        self.assertIn("score_breakdown", candidate)
        self.assertIn("visual_breakdown", candidate)
        self.assertGreater(candidate["score_breakdown"]["energy"], 0)
        self.assertGreater(candidate["visual_breakdown"]["shot_type"], 0)
        self.assertGreater(candidate["visual_breakdown"]["action"], 0)
        self.assertGreater(candidate["visual_breakdown"]["face"], 0)
        self.assertGreater(candidate["visual_breakdown"]["quality"], 0)
        self.assertTrue(candidate["score_breakdown"]["topic_boundary"])
        self.assertEqual(
            candidate["score"],
            candidate["score_breakdown"]["total"],
        )

    def test_score_candidates_can_suppress_overlapping_clips(self) -> None:
        candidates = score_moments.score_candidates(
            moments_body={
                "moments": [
                    {
                        "id": "strong",
                        "kind": "hook",
                        "start_s": 10.0,
                        "end_s": 50.0,
                        "score": 0.9,
                        "note": "the real hook",
                    },
                    {
                        "id": "duplicate",
                        "kind": "story",
                        "start_s": 15.0,
                        "end_s": 45.0,
                        "score": 0.85,
                        "note": "same section",
                    },
                    {
                        "id": "separate",
                        "kind": "story",
                        "start_s": 80.0,
                        "end_s": 120.0,
                        "score": 0.7,
                        "note": "separate payoff",
                    },
                ]
            },
            energy_body={"windows": []},
            transcript_body={
                "segments": [
                    {
                        "start_s": 10.0,
                        "end_s": 50.0,
                        "text": "This is the real hook.",
                    },
                    {
                        "start_s": 80.0,
                        "end_s": 120.0,
                        "text": "This is the separate payoff.",
                    },
                ]
            },
            limit=5,
            max_overlap_ratio=0.5,
        )

        self.assertEqual([c["moment_id"] for c in candidates], ["strong", "separate"])

    def test_score_candidates_rejects_invalid_overlap_ratio(self) -> None:
        with self.assertRaisesRegex(ValueError, "max_overlap_ratio"):
            score_moments.score_candidates(
                moments_body={"moments": []},
                energy_body={"windows": []},
                max_overlap_ratio=1.5,
            )


if __name__ == "__main__":
    unittest.main()
