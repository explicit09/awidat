#!/usr/bin/env python3
"""Tests for the cross-source supercut merger."""

from __future__ import annotations

import unittest

import merge_sources


def cand(score, kind="story", speaker="A", hook=False):
    return {
        "moment_id": f"m-{score}",
        "kind": kind,
        "start_s": 0.0,
        "end_s": 30.0,
        "score": score,
        "speaker_id": speaker,
        "hook_signal": hook,
    }


class MergeSourcesTest(unittest.TestCase):
    def test_cold_click_bar_drops_weak_candidates(self) -> None:
        sources = [
            ("raw/ep-01.mp4", [cand(90), cand(40)]),
            ("raw/ep-02.mp4", [cand(70), cand(20)]),
        ]
        result = merge_sources.select(
            sources,
            min_score=60.0,
            target_count=10,
            per_source_cap=None,
            balance_speakers=False,
        )
        self.assertEqual(result["stats"]["selected"], 2)
        self.assertEqual(result["stats"]["rejected_below_bar"], 2)
        self.assertTrue(all(c["score"] >= 60.0 for c in result["spine"]))

    def test_per_source_cap_prevents_one_episode_dominating(self) -> None:
        sources = [
            ("raw/ep-01.mp4", [cand(95), cand(94), cand(93)]),
            ("raw/ep-02.mp4", [cand(80)]),
        ]
        result = merge_sources.select(
            sources,
            min_score=60.0,
            target_count=10,
            per_source_cap=2,
            balance_speakers=False,
        )
        per_source = result["stats"]["per_source"]
        self.assertEqual(per_source["raw/ep-01.mp4"], 2)
        self.assertEqual(per_source["raw/ep-02.mp4"], 1)

    def test_hook_first_ordering(self) -> None:
        sources = [
            ("raw/ep-01.mp4", [cand(70, hook=True), cand(99)]),
        ]
        result = merge_sources.select(
            sources,
            min_score=60.0,
            target_count=10,
            per_source_cap=None,
            balance_speakers=False,
        )
        self.assertTrue(result["spine"][0]["hook_signal"])
        self.assertEqual(result["spine"][0]["spine_position"], 0)

    def test_speaker_balance_spreads_selection(self) -> None:
        sources = [
            ("raw/ep-01.mp4", [cand(99, speaker="A"), cand(98, speaker="A"),
                               cand(97, speaker="A"), cand(96, speaker="A")]),
            ("raw/ep-02.mp4", [cand(70, speaker="B")]),
        ]
        result = merge_sources.select(
            sources,
            min_score=60.0,
            target_count=4,
            per_source_cap=None,
            balance_speakers=True,
        )
        per_speaker = result["stats"]["per_speaker"]
        # With two speakers and target 4, ceiling = ceil(4/2)+1 = 3, so A is
        # capped below taking all four slots and B gets a seat.
        self.assertIn("B", per_speaker)
        self.assertLessEqual(per_speaker["A"], 3)

    def test_quota_ignores_speakers_with_no_eligible_clips(self) -> None:
        # Speaker B only has below-bar clips, so it must not count toward the
        # speaker quota denominator and starve speaker A's eligible clips.
        sources = [
            ("raw/ep-01.mp4", [cand(99, speaker="A"), cand(98, speaker="A"),
                               cand(97, speaker="A"), cand(96, speaker="A")]),
            ("raw/ep-02.mp4", [cand(40, speaker="B"), cand(30, speaker="B")]),
        ]
        result = merge_sources.select(
            sources,
            min_score=60.0,
            target_count=4,
            per_source_cap=None,
            balance_speakers=True,
        )
        # Only A is eligible -> ceiling = ceil(4/1)+1 = 5, so all four A clips fit.
        self.assertEqual(result["stats"]["selected"], 4)
        self.assertEqual(result["stats"]["per_speaker"]["A"], 4)

    def test_overlapping_moments_are_deduplicated(self) -> None:
        # Two above-bar variants of the same moment (shared moment_id) must not
        # both fill spine slots and repeat the clip.
        dup = [
            {"moment_id": "m-shared", "kind": "story", "start_s": 0.0,
             "end_s": 30.0, "score": 99, "speaker_id": "A"},
            {"moment_id": "m-shared", "kind": "story", "start_s": 0.0,
             "end_s": 30.0, "score": 98, "speaker_id": "A"},
        ]
        result = merge_sources.select(
            [("raw/ep-01.mp4", dup)],
            min_score=60.0,
            target_count=4,
            per_source_cap=None,
            balance_speakers=False,
        )
        self.assertEqual(result["stats"]["selected"], 1)

    def test_load_source_inline_asset_override(self) -> None:
        import json
        import tempfile

        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
            json.dump({"candidates": [cand(80)]}, f)
            path = f.name
        asset, candidates = merge_sources.load_source(f"raw/ep-09.mp4={path}")
        self.assertEqual(asset, "raw/ep-09.mp4")
        self.assertEqual(len(candidates), 1)


if __name__ == "__main__":
    unittest.main()
