#!/usr/bin/env python3
"""Tests for transcript selection group planning."""

from __future__ import annotations

import unittest

from transcript_selection_groups import (
    build_selection_pack,
    render_markdown,
)


TRANSCRIPT = {
    "segments": [
        {"start_s": 0.0, "end_s": 1.0, "text": "Cold open line", "speaker_id": "A"},
        {"start_s": 1.0, "end_s": 2.0, "text": "continues the same thought", "speaker_id": "A"},
        {"start_s": 4.0, "end_s": 5.0, "text": "second selected line", "speaker_id": "B"},
        {"start_s": 8.0, "end_s": 9.0, "text": "unrelated line", "speaker_id": "C"},
    ],
    "transcript_groups": {
        "hook_001": {
            "name": "Hook Candidates",
            "notes": "strong opener about product risk",
            "time_intervals": [
                {"start": 0.0, "end": 2.0},
                {"start": 4.0, "end": 5.0},
            ],
        },
        "quiet_001": {
            "name": "Quiet Setup",
            "notes": "slow setup",
            "time_intervals": [{"start": 8.0, "end": 9.0}],
        },
    },
}


class TranscriptSelectionGroupsTest(unittest.TestCase):
    def test_selects_named_group_and_merges_contiguous_intervals(self) -> None:
        pack = build_selection_pack(
            [("raw-a", TRANSCRIPT)],
            group_filters=["hook"],
            query=None,
        )

        self.assertEqual(pack["status"], "ready")
        group = pack["groups"][0]
        self.assertEqual(group["source_label"], "raw-a")
        self.assertEqual(group["group_name"], "Hook Candidates")
        self.assertEqual(group["merged_intervals"], [
            {"start_s": 0.0, "end_s": 2.0},
            {"start_s": 4.0, "end_s": 5.0},
        ])
        self.assertEqual(group["selected_segments"][0]["text"], "Cold open line")
        self.assertEqual(group["selected_segments"][1]["text"], "continues the same thought")
        self.assertEqual(group["duration_s"], 3.0)

    def test_query_matches_group_notes_and_segments(self) -> None:
        pack = build_selection_pack(
            [("raw-a", TRANSCRIPT)],
            group_filters=[],
            query="product risk opener",
        )

        self.assertEqual([g["group_name"] for g in pack["groups"]], ["Hook Candidates"])
        self.assertIn("strong opener", pack["groups"][0]["notes"])
        self.assertIn("Cold open line", pack["groups"][0]["text"])

    def test_blocks_empty_group_selection(self) -> None:
        pack = build_selection_pack(
            [("raw-a", TRANSCRIPT)],
            group_filters=["missing"],
            query=None,
        )

        self.assertEqual(pack["status"], "blocked")
        self.assertEqual(pack["groups"], [])
        self.assertIn("No transcript groups matched", pack["warnings"][0])

    def test_renders_markdown_with_time_ranges_and_notes(self) -> None:
        pack = build_selection_pack(
            [("raw-a", TRANSCRIPT)],
            group_filters=["hook"],
            query=None,
        )

        rendered = render_markdown(pack)
        self.assertIn("## raw-a / Hook Candidates", rendered)
        self.assertIn("Notes: strong opener about product risk", rendered)
        self.assertIn("[0.00-2.00]", rendered)
        self.assertIn("Cold open line continues the same thought", rendered)


if __name__ == "__main__":
    unittest.main()
