#!/usr/bin/env python3
"""Tests for word-level caption progress evidence."""

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


class CaptionProgressEvidenceTests(unittest.TestCase):
    def test_builds_word_level_karaoke_progress_frames(self) -> None:
        evidence = load_script("caption_progress_evidence")
        transcript = {
            "words": [
                {"word": "Build", "start_s": 0.0, "end_s": 0.4, "speaker": "A"},
                {"word": "better", "start_s": 0.45, "end_s": 0.9, "speaker": "A"},
                {"word": "captions", "start_s": 0.95, "end_s": 1.4, "speaker": "A"},
            ]
        }

        report = evidence.build_progress_report(
            transcript,
            sample_times=[0.0, 0.5, 1.2, 1.6],
            style="karaoke_fill",
        )

        self.assertEqual(report["status"], "ready")
        self.assertEqual(report["style"], "karaoke_fill")
        self.assertEqual(report["word_count"], 3)
        self.assertEqual(report["frames"][0]["active_word"], "Build")
        self.assertEqual(report["frames"][1]["active_word"], "better")
        self.assertEqual(report["frames"][2]["active_word"], "captions")
        self.assertIsNone(report["frames"][3]["active_word"])
        self.assertEqual(report["frames"][1]["words"][0]["state"], "completed")
        self.assertEqual(report["frames"][1]["words"][1]["state"], "active")
        self.assertEqual(report["frames"][1]["words"][2]["state"], "pending")

    def test_rejects_unsorted_or_invalid_word_timing(self) -> None:
        evidence = load_script("caption_progress_evidence")

        report = evidence.build_progress_report(
            {
                "words": [
                    {"word": "late", "start_s": 1.0, "end_s": 1.2},
                    {"word": "early", "start_s": 0.5, "end_s": 0.8},
                    {"word": "bad", "start_s": 1.4, "end_s": 1.3},
                ]
            },
            sample_times=[0.5],
            style="word_highlight",
        )

        self.assertEqual(report["status"], "blocked")
        codes = {issue["code"] for issue in report["issues"]}
        self.assertIn("unsorted_words", codes)
        self.assertIn("invalid_word_timing", codes)

    def test_cli_emits_progress_report(self) -> None:
        transcript = {
            "words": [
                {"word": "One", "start_s": 0.0, "end_s": 0.3},
                {"word": "beat", "start_s": 0.35, "end_s": 0.7},
            ]
        }
        with tempfile.NamedTemporaryFile("w", suffix=".json") as handle:
            json.dump(transcript, handle)
            handle.flush()

            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT_DIR / "caption_progress_evidence.py"),
                    "--transcript",
                    handle.name,
                    "--sample-times",
                    "0.1,0.5",
                    "--style",
                    "word_highlight",
                ],
                check=True,
                capture_output=True,
                text=True,
            )

        payload = json.loads(result.stdout)
        self.assertEqual(payload["status"], "ready")
        self.assertEqual(payload["frames"][0]["active_word"], "One")
        self.assertEqual(payload["frames"][1]["active_word"], "beat")


if __name__ == "__main__":
    unittest.main()
