#!/usr/bin/env python3
"""Tests for short-form transcript phrase and caption planning helpers."""

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


class TranscriptPhraseTests(unittest.TestCase):
    def test_groups_on_punctuation_silence_and_speaker_change(self) -> None:
        transcript_phrases = load_script("transcript_phrases")
        items = [
            {"word": "We", "start_s": 0.00, "end_s": 0.20, "speaker": "S1"},
            {"word": "ship", "start_s": 0.24, "end_s": 0.50, "speaker": "S1"},
            {"word": "today.", "start_s": 0.55, "end_s": 0.82, "speaker": "S1"},
            {"word": "Then", "start_s": 1.62, "end_s": 1.82, "speaker": "S1"},
            {"word": "listen", "start_s": 1.86, "end_s": 2.20, "speaker": "S2"},
        ]

        phrases = transcript_phrases.group_words_into_phrases(
            items,
            max_words=4,
            max_gap_s=0.5,
        )

        self.assertEqual(
            [(p["text"], p["speaker"]) for p in phrases],
            [("We ship today.", "S1"), ("Then", "S1"), ("listen", "S2")],
        )
        self.assertEqual(phrases[0]["start_s"], 0.0)
        self.assertEqual(phrases[0]["end_s"], 0.82)

    def test_caption_phrases_apply_hot_style_and_mobile_metadata(self) -> None:
        caption_plan = load_script("caption_plan")
        items = [
            {"word": "Quiet", "start_s": 0.0, "end_s": 0.2, "speaker": "A"},
            {"word": "setup", "start_s": 0.22, "end_s": 0.5, "speaker": "A"},
            {"word": "Big", "start_s": 1.1, "end_s": 1.3, "speaker": "A"},
            {"word": "payoff", "start_s": 1.35, "end_s": 1.8, "speaker": "A"},
        ]

        phrases = caption_plan.build_caption_phrases(
            items,
            max_words=4,
            hot_start_s=1.0,
            hot_end_s=1.9,
        )

        self.assertEqual([p["text"] for p in phrases], ["Quiet setup", "Big payoff"])
        self.assertEqual(phrases[0]["safe_area"], "mobile")
        self.assertEqual(phrases[0]["color"], "#FFFFFF")
        self.assertEqual(phrases[1]["color"], "#FFD400")
        self.assertEqual(phrases[1]["font_weight"], "bold")

    def test_caption_style_presets_apply_named_visual_contracts(self) -> None:
        caption_plan = load_script("caption_plan")
        items = [
            {"word": "Big", "start_s": 0.0, "end_s": 0.2, "speaker": "A"},
            {"word": "claim", "start_s": 0.22, "end_s": 0.5, "speaker": "A"},
        ]

        phrases = caption_plan.build_caption_phrases(
            items,
            max_words=4,
            style="impact",
        )

        self.assertEqual(phrases[0]["style"], "impact")
        self.assertEqual(phrases[0]["position"], "center")
        self.assertEqual(phrases[0]["font_size"], 64)
        self.assertEqual(phrases[0]["font_weight"], "bold")
        self.assertEqual(phrases[0]["animation"], "pop_in")
        self.assertEqual(phrases[0]["background"], "transparent")

    def test_caption_hot_range_overrides_preserve_style_contract(self) -> None:
        caption_plan = load_script("caption_plan")
        items = [
            {"word": "Quiet", "start_s": 0.0, "end_s": 0.2, "speaker": "A"},
            {"word": "setup", "start_s": 0.22, "end_s": 0.5, "speaker": "A"},
            {"word": "Big", "start_s": 1.1, "end_s": 1.3, "speaker": "A"},
            {"word": "payoff", "start_s": 1.35, "end_s": 1.8, "speaker": "A"},
        ]

        phrases = caption_plan.build_caption_phrases(
            items,
            max_words=4,
            hot_start_s=1.0,
            hot_end_s=1.9,
            style="boxed",
        )

        self.assertEqual(phrases[0]["style"], "boxed")
        self.assertEqual(phrases[0]["background"], "rgba(0,0,0,0.72)")
        self.assertEqual(phrases[0]["color"], "#FFFFFF")
        self.assertEqual(phrases[1]["style"], "boxed")
        self.assertEqual(phrases[1]["color"], "#FFD400")
        self.assertEqual(phrases[1]["font_weight"], "bold")
        self.assertEqual(phrases[1]["background"], "rgba(0,0,0,0.72)")

    def test_segment_fallback_preserves_sentence_punctuation(self) -> None:
        transcript_phrases = load_script("transcript_phrases")
        body = {
            "segments": [
                {
                    "text": "First point. Second point",
                    "start_s": 10.0,
                    "end_s": 12.0,
                    "speaker": "Host",
                }
            ]
        }

        phrases = transcript_phrases.group_words_into_phrases(body, max_words=6)

        self.assertEqual([p["text"] for p in phrases], ["First point.", "Second point"])

    def test_phrase_preset_breaks_on_duration_before_word_limit(self) -> None:
        transcript_phrases = load_script("transcript_phrases")
        items = [
            {"word": "Slow", "start_s": 0.0, "end_s": 0.7, "speaker": "A"},
            {"word": "setup", "start_s": 0.75, "end_s": 1.45, "speaker": "A"},
            {"word": "keeps", "start_s": 1.5, "end_s": 2.2, "speaker": "A"},
            {"word": "moving", "start_s": 2.25, "end_s": 2.95, "speaker": "A"},
            {"word": "forward", "start_s": 3.0, "end_s": 3.7, "speaker": "A"},
        ]

        phrases = transcript_phrases.group_words_into_phrases(
            items,
            phrase_preset="short",
        )

        self.assertEqual(
            [p["text"] for p in phrases],
            ["Slow setup keeps", "moving forward"],
        )

    def test_phrase_preset_breaks_on_soft_punctuation_after_target_duration(self) -> None:
        transcript_phrases = load_script("transcript_phrases")
        items = [
            {"word": "First", "start_s": 0.0, "end_s": 0.5, "speaker": "A"},
            {"word": "idea,", "start_s": 0.55, "end_s": 1.45, "speaker": "A"},
            {"word": "second", "start_s": 1.5, "end_s": 1.9, "speaker": "A"},
            {"word": "lands", "start_s": 1.95, "end_s": 2.25, "speaker": "A"},
        ]

        phrases = transcript_phrases.group_words_into_phrases(
            items,
            phrase_preset="short",
        )

        self.assertEqual([p["text"] for p in phrases], ["First idea,", "second lands"])

    def test_caption_plan_accepts_phrase_preset(self) -> None:
        caption_plan = load_script("caption_plan")
        items = [
            {"word": "One", "start_s": 0.0, "end_s": 0.25, "speaker": "A"},
            {"word": "quick", "start_s": 0.3, "end_s": 0.55, "speaker": "A"},
            {"word": "beat", "start_s": 0.6, "end_s": 0.85, "speaker": "A"},
            {"word": "Next", "start_s": 1.35, "end_s": 1.55, "speaker": "A"},
            {"word": "line", "start_s": 1.6, "end_s": 1.9, "speaker": "A"},
        ]

        phrases = caption_plan.build_caption_phrases(
            items,
            phrase_preset="short",
        )

        self.assertEqual([p["text"] for p in phrases], ["One quick beat", "Next line"])

    def test_caption_plan_exports_srt_with_timestamping_and_sanitized_arrows(self) -> None:
        caption_plan = load_script("caption_plan")
        phrases = [
            {"text": "First --> point", "start_s": 0.0, "end_s": 0.64},
            {"text": "Past an hour", "start_s": 3661.2, "end_s": 3662.3456},
        ]

        srt = caption_plan.build_srt(phrases)

        self.assertIn("00:00:00,000 --> 00:00:00,640", srt)
        self.assertIn("01:01:01,200 --> 01:01:02,346", srt)
        self.assertIn("First -> point", srt)
        self.assertTrue(srt.endswith("\n"))

        with self.assertRaisesRegex(ValueError, "end_s"):
            caption_plan.build_srt([{"text": "bad", "start_s": 2.0, "end_s": 1.0}])

    def test_caption_plan_exports_vtt_with_timestamping_and_sanitized_arrows(self) -> None:
        caption_plan = load_script("caption_plan")
        phrases = [
            {"text": "First --> point", "start_s": 0.0, "end_s": 0.64},
            {"text": "Past an hour", "start_s": 3661.2, "end_s": 3662.3456},
        ]

        vtt = caption_plan.build_vtt(phrases)

        self.assertTrue(vtt.startswith("WEBVTT\n\n"))
        self.assertIn("00:00:00.000 --> 00:00:00.640", vtt)
        self.assertIn("01:01:01.200 --> 01:01:02.346", vtt)
        self.assertIn("First -> point", vtt)
        self.assertTrue(vtt.endswith("\n"))

        with self.assertRaisesRegex(ValueError, "end_s"):
            caption_plan.build_vtt([{"text": "bad", "start_s": 2.0, "end_s": 1.0}])

    def test_caption_plan_cli_can_emit_srt(self) -> None:
        transcript = {
            "words": [
                {"word": "One", "start_s": 0.0, "end_s": 0.2, "speaker": "A"},
                {"word": "line", "start_s": 0.25, "end_s": 0.5, "speaker": "A"},
            ]
        }
        with tempfile.NamedTemporaryFile("w", suffix=".json") as handle:
            json.dump(transcript, handle)
            handle.flush()

            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT_DIR / "caption_plan.py"),
                    "--transcript",
                    handle.name,
                    "--output-format",
                    "srt",
                ],
                check=True,
                capture_output=True,
                text=True,
            )

        self.assertIn("1\n00:00:00,000 --> 00:00:00,600", result.stdout)
        self.assertIn("One line", result.stdout)

    def test_caption_plan_cli_can_emit_vtt(self) -> None:
        transcript = {
            "words": [
                {"word": "One", "start_s": 0.0, "end_s": 0.2, "speaker": "A"},
                {"word": "line", "start_s": 0.25, "end_s": 0.5, "speaker": "A"},
            ]
        }
        with tempfile.NamedTemporaryFile("w", suffix=".json") as handle:
            json.dump(transcript, handle)
            handle.flush()

            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT_DIR / "caption_plan.py"),
                    "--transcript",
                    handle.name,
                    "--output-format",
                    "vtt",
                ],
                check=True,
                capture_output=True,
                text=True,
            )

        self.assertTrue(result.stdout.startswith("WEBVTT\n\n"))
        self.assertIn("1\n00:00:00.000 --> 00:00:00.600", result.stdout)
        self.assertIn("One line", result.stdout)

    def test_packed_transcript_markdown_has_source_and_timed_rows(self) -> None:
        pack_transcript = load_script("pack_transcript")
        transcript = {
            "words": [
                {"word": "First", "start_s": 3.0, "end_s": 3.2, "speaker": "Host"},
                {"word": "point.", "start_s": 3.22, "end_s": 3.6, "speaker": "Host"},
                {"word": "Reply", "start_s": 4.3, "end_s": 4.7, "speaker": "Guest"},
            ]
        }

        markdown = pack_transcript.build_packed_transcript(
            [("take-a", transcript)],
            max_words=4,
            max_gap_s=0.5,
        )

        self.assertIn("# Packed transcripts", markdown)
        self.assertIn("## take-a", markdown)
        self.assertIn("[3.00-3.60] Host First point.", markdown)
        self.assertIn("[4.30-4.70] Guest Reply", markdown)


if __name__ == "__main__":
    unittest.main()
