#!/usr/bin/env python3
"""Tests for transcript-first packed transcript packets."""

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


class PackTranscriptTests(unittest.TestCase):
    def test_json_packet_is_source_labeled_and_token_bounded(self) -> None:
        pack_transcript = load_script("pack_transcript")
        packet = pack_transcript.build_transcript_packet(
            [
                (
                    "take-a",
                    {
                        "asset_sha256": "abc",
                        "source_asset_sha256": "abc",
                        "words": [
                            {"word": "First", "start_s": 0.0, "end_s": 0.2, "speaker": "A"},
                            {"word": "point.", "start_s": 0.2, "end_s": 0.6, "speaker": "A"},
                            {"word": "Second", "start_s": 2.0, "end_s": 2.3, "speaker": "B"},
                            {"word": "point.", "start_s": 2.3, "end_s": 2.7, "speaker": "B"},
                        ],
                    },
                )
            ],
            max_words=4,
            max_gap_s=0.5,
            max_chars=72,
        )

        self.assertEqual(packet["status"], "truncated")
        self.assertEqual(packet["source_count"], 1)
        self.assertEqual(packet["sources"][0]["label"], "take-a")
        self.assertEqual(packet["sources"][0]["freshness"], "fresh")
        self.assertLessEqual(len(packet["markdown"]), 72)

    def test_cli_can_emit_json_packet(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            transcript = Path(tmp) / "take.json"
            transcript.write_text(json.dumps({
                "data": {
                    "words": [
                        {"word": "Hello", "start_s": 1.0, "end_s": 1.2},
                        {"word": "world.", "start_s": 1.2, "end_s": 1.6},
                    ]
                }
            }))

            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT_DIR / "pack_transcript.py"),
                    "--transcript",
                    str(transcript),
                    "--source-label",
                    "take-a",
                    "--output-format",
                    "json",
                ],
                check=True,
                capture_output=True,
                text=True,
            )

        packet = json.loads(result.stdout)
        self.assertEqual(packet["status"], "ready")
        self.assertEqual(packet["sources"][0]["label"], "take-a")
        self.assertIn("[1.00-1.60]", packet["markdown"])


if __name__ == "__main__":
    unittest.main()
