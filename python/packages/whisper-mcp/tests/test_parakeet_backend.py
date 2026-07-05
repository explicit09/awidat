from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


MODULE = Path(__file__).parents[1] / "src" / "whisper_mcp" / "parakeet_backend.py"
SPEC = importlib.util.spec_from_file_location("parakeet_backend", MODULE)
assert SPEC is not None and SPEC.loader is not None
parakeet_backend = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(parakeet_backend)


class MergeTokensToWords(unittest.TestCase):
    """Parakeet emits subword pieces; a leading space starts a new word."""

    def test_merges_pieces_into_words_on_leading_space(self) -> None:
        tokens = [
            (" Hello", 0.0, 0.2),
            (" W", 0.2, 0.3),
            ("orld", 0.3, 0.45),
        ]
        words = parakeet_backend.merge_tokens_to_words(tokens)
        self.assertEqual(
            words,
            [
                {"text": "Hello", "start_s": 0.0, "end_s": 0.2, "speaker_id": None},
                {"text": "World", "start_s": 0.2, "end_s": 0.45, "speaker_id": None},
            ],
        )

    def test_first_piece_starts_word_even_without_space(self) -> None:
        words = parakeet_backend.merge_tokens_to_words([("Hey", 0.1, 0.3)])
        self.assertEqual(len(words), 1)
        self.assertEqual(words[0]["text"], "Hey")

    def test_skips_whitespace_only_pieces(self) -> None:
        tokens = [(" Hi", 0.0, 0.1), ("  ", 0.1, 0.2), (" there", 0.2, 0.3)]
        words = parakeet_backend.merge_tokens_to_words(tokens)
        self.assertEqual([w["text"] for w in words], ["Hi", "there"])

    def test_empty_input(self) -> None:
        self.assertEqual(parakeet_backend.merge_tokens_to_words([]), [])


class AssignSpeakers(unittest.TestCase):
    """Words get speaker ids from diarization turns by midpoint containment.

    Raw senko labels are arbitrary; they map to SPEAKER_00/SPEAKER_01/…
    in order of first appearance on the timeline (WhisperX convention).
    """

    TURNS = [
        {"start": 0.0, "end": 2.0, "speaker": "3"},
        {"start": 2.5, "end": 4.0, "speaker": "1"},
        {"start": 5.0, "end": 6.0, "speaker": "3"},
    ]

    def _word(self, start: float, end: float) -> dict:
        return {"text": "w", "start_s": start, "end_s": end, "speaker_id": None}

    def test_labels_by_first_appearance_order(self) -> None:
        words = [self._word(0.5, 1.0), self._word(2.6, 3.0), self._word(5.2, 5.5)]
        out = parakeet_backend.assign_speakers(words, self.TURNS)
        self.assertEqual(
            [w["speaker_id"] for w in out],
            ["SPEAKER_00", "SPEAKER_01", "SPEAKER_00"],
        )

    def test_word_outside_any_turn_gets_none(self) -> None:
        out = parakeet_backend.assign_speakers([self._word(10.0, 10.4)], self.TURNS)
        self.assertIsNone(out[0]["speaker_id"])

    def test_tolerance_covers_turn_boundary_jitter(self) -> None:
        # midpoint 4.1 is 0.1s past the turn end — within the 0.25s tolerance
        out = parakeet_backend.assign_speakers([self._word(4.0, 4.2)], self.TURNS)
        self.assertEqual(out[0]["speaker_id"], "SPEAKER_01")

    def test_does_not_mutate_input(self) -> None:
        words = [self._word(0.5, 1.0)]
        parakeet_backend.assign_speakers(words, self.TURNS)
        self.assertIsNone(words[0]["speaker_id"])

    def test_no_turns_leaves_all_unlabeled(self) -> None:
        out = parakeet_backend.assign_speakers([self._word(0.5, 1.0)], [])
        self.assertIsNone(out[0]["speaker_id"])


class SpeakerTotals(unittest.TestCase):
    def test_aggregates_speech_time_per_speaker(self) -> None:
        words = [
            {"text": "a", "start_s": 0.0, "end_s": 1.0, "speaker_id": "SPEAKER_00"},
            {"text": "b", "start_s": 1.0, "end_s": 1.5, "speaker_id": "SPEAKER_01"},
            {"text": "c", "start_s": 2.0, "end_s": 2.25, "speaker_id": "SPEAKER_00"},
            {"text": "d", "start_s": 3.0, "end_s": 3.5, "speaker_id": None},
        ]
        self.assertEqual(
            parakeet_backend.speaker_totals(words),
            [
                {"id": "SPEAKER_00", "total_speech_s": 1.25},
                {"id": "SPEAKER_01", "total_speech_s": 0.5},
            ],
        )

    def test_empty(self) -> None:
        self.assertEqual(parakeet_backend.speaker_totals([]), [])



class BackendSelection(unittest.TestCase):
    """`_transcribe` routes to the parakeet handler when selected/available."""

    def _load_whisper_mcp(self):
        module_path = Path(__file__).parents[1] / "src" / "whisper_mcp" / "__init__.py"
        spec = importlib.util.spec_from_file_location("whisper_mcp_sel", module_path)
        assert spec is not None and spec.loader is not None
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        return mod

    def test_explicit_parakeet_backend_dispatches(self) -> None:
        wm = self._load_whisper_mcp()
        wm.BACKEND = "parakeet"
        sentinel = {"model": "parakeet-mlx:test"}
        wm._handle_parakeet = lambda req: sentinel
        req = type("Req", (), {"asset_path": "/tmp/x.mov"})()
        self.assertIs(wm._transcribe(req), sentinel)

    def test_auto_chain_prefers_parakeet_when_available(self) -> None:
        wm = self._load_whisper_mcp()
        wm.BACKEND = "auto"
        wm.DEEPGRAM_API_KEY = None
        wm._can_use_parakeet_backend = lambda: True
        sentinel = {"model": "parakeet-mlx:test"}
        wm._handle_parakeet = lambda req: sentinel
        req = type("Req", (), {"asset_path": "/tmp/x.mov"})()
        self.assertIs(wm._transcribe(req), sentinel)

    def test_auto_chain_falls_through_when_parakeet_unavailable(self) -> None:
        wm = self._load_whisper_mcp()
        wm.BACKEND = "auto"
        wm.DEEPGRAM_API_KEY = None
        wm._can_use_parakeet_backend = lambda: False
        wm._can_use_whispercpp_backend = lambda: False
        marker = {"model": "whisperx"}
        wm._handle_whisperx = lambda req: marker
        req = type("Req", (), {"asset_path": "/tmp/x.mov"})()
        self.assertIs(wm._transcribe(req), marker)

if __name__ == "__main__":
    unittest.main()
