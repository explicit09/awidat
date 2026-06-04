from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


MODULE = Path(__file__).parents[1] / "src" / "whisper_mcp" / "__init__.py"
SPEC = importlib.util.spec_from_file_location("whisper_mcp", MODULE)
assert SPEC is not None and SPEC.loader is not None
whisper_mcp = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(whisper_mcp)


def _diarized_body() -> dict:
    """A transcript body shaped like the backends emit it.

    SPEAKER_00 talks first but less; SPEAKER_01 talks more overall.
    """
    return {
        "language": "en",
        "model": "test",
        "words": [
            {"text": "Hello", "start_s": 0.0, "end_s": 0.5, "speaker_id": "SPEAKER_00"},
            {"text": "there", "start_s": 0.5, "end_s": 1.0, "speaker_id": "SPEAKER_00"},
            {"text": "Hi", "start_s": 1.0, "end_s": 3.0, "speaker_id": "SPEAKER_01"},
            {"text": "everyone", "start_s": 3.0, "end_s": 5.0, "speaker_id": "SPEAKER_01"},
            {"text": "today", "start_s": 5.0, "end_s": 6.0, "speaker_id": "SPEAKER_02"},
        ],
        "segments": [
            {"text": "Hello there", "start_s": 0.0, "end_s": 1.0, "speaker_id": "SPEAKER_00"},
            {"text": "Hi everyone", "start_s": 1.0, "end_s": 5.0, "speaker_id": "SPEAKER_01"},
            {"text": "today", "start_s": 5.0, "end_s": 6.0, "speaker_id": "SPEAKER_02"},
        ],
        "speakers": [
            {"id": "SPEAKER_00", "total_speech_s": 1.0},
            {"id": "SPEAKER_01", "total_speech_s": 4.0},
            {"id": "SPEAKER_02", "total_speech_s": 1.0},
        ],
        "diarized": True,
    }


class SpeakerNamesTest(unittest.TestCase):
    def _attach_with_map(self, raw_map: dict | None) -> dict:
        body = _diarized_body()
        with tempfile.TemporaryDirectory() as tmp:
            asset = Path(tmp) / "clip.mp4"
            asset.write_bytes(b"")
            if raw_map is not None:
                (Path(tmp) / "speaker_names.json").write_text(
                    json.dumps(raw_map), encoding="utf-8"
                )
            return whisper_mcp._attach_speaker_names(body, str(asset))

    def test_direct_id_mapping(self) -> None:
        out = self._attach_with_map({"SPEAKER_00": "Tadiwa", "SPEAKER_01": "Elvis"})
        self.assertEqual(out["words"][0]["speaker_name"], "Tadiwa")
        self.assertEqual(out["words"][2]["speaker_name"], "Elvis")
        names = {s["id"]: s["name"] for s in out["speakers"]}
        self.assertEqual(names["SPEAKER_00"], "Tadiwa")
        self.assertEqual(names["SPEAKER_01"], "Elvis")

    def test_unmapped_speaker_stays_null(self) -> None:
        out = self._attach_with_map({"SPEAKER_00": "Tadiwa"})
        # SPEAKER_02 has no entry -> name must be null, not missing.
        s02_words = [w for w in out["words"] if w["speaker_id"] == "SPEAKER_02"]
        self.assertTrue(s02_words)
        for w in s02_words:
            self.assertIn("speaker_name", w)
            self.assertIsNone(w["speaker_name"])
        names = {s["id"]: s["name"] for s in out["speakers"]}
        self.assertIsNone(names["SPEAKER_02"])

    def test_no_map_file_all_null_schema_preserved(self) -> None:
        out = self._attach_with_map(None)
        for w in out["words"]:
            self.assertIsNone(w["speaker_name"])
        for seg in out["segments"]:
            self.assertIsNone(seg["speaker_name"])
        for s in out["speakers"]:
            self.assertIsNone(s["name"])
        # Original fields are untouched.
        self.assertEqual(out["words"][0]["text"], "Hello")
        self.assertEqual(out["words"][0]["speaker_id"], "SPEAKER_00")

    def test_by_rank_orders_by_total_speech_time(self) -> None:
        # SPEAKER_01 talks most (4.0s) -> primary -> "Primary".
        out = self._attach_with_map({"@by_rank": ["Primary", "Secondary"]})
        names = {s["id"]: s["name"] for s in out["speakers"]}
        self.assertEqual(names["SPEAKER_01"], "Primary")
        # SPEAKER_00 and SPEAKER_02 tie at 1.0; tiebreak is id asc -> SPEAKER_00.
        self.assertEqual(names["SPEAKER_00"], "Secondary")
        self.assertIsNone(names["SPEAKER_02"])

    def test_by_order_uses_first_to_speak(self) -> None:
        out = self._attach_with_map({"@by_order": ["Host", "Guest"]})
        names = {s["id"]: s["name"] for s in out["speakers"]}
        # SPEAKER_00 speaks first, SPEAKER_01 second.
        self.assertEqual(names["SPEAKER_00"], "Host")
        self.assertEqual(names["SPEAKER_01"], "Guest")
        self.assertIsNone(names["SPEAKER_02"])

    def test_direct_id_wins_over_ordering_hint(self) -> None:
        out = self._attach_with_map(
            {"@by_rank": ["A", "B", "C"], "SPEAKER_01": "Pinned"}
        )
        names = {s["id"]: s["name"] for s in out["speakers"]}
        # Direct id pins SPEAKER_01; rank still fills the others, skipping the
        # already-pinned id.
        self.assertEqual(names["SPEAKER_01"], "Pinned")
        self.assertEqual(names["SPEAKER_00"], "B")

    def test_explicit_env_path(self) -> None:
        body = _diarized_body()
        with tempfile.TemporaryDirectory() as tmp:
            asset = Path(tmp) / "clip.mp4"
            asset.write_bytes(b"")
            map_path = Path(tmp) / "names_elsewhere.json"
            map_path.write_text(json.dumps({"SPEAKER_00": "Tadiwa"}), encoding="utf-8")
            original = whisper_mcp.SPEAKER_NAMES_PATH
            whisper_mcp.SPEAKER_NAMES_PATH = str(map_path)
            try:
                out = whisper_mcp._attach_speaker_names(body, str(asset))
            finally:
                whisper_mcp.SPEAKER_NAMES_PATH = original
        self.assertEqual(out["words"][0]["speaker_name"], "Tadiwa")

    def test_ancestor_dir_lookup(self) -> None:
        body = _diarized_body()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "speaker_names.json").write_text(
                json.dumps({"SPEAKER_00": "Tadiwa"}), encoding="utf-8"
            )
            nested = root / "media" / "raw"
            nested.mkdir(parents=True)
            asset = nested / "clip.mp4"
            asset.write_bytes(b"")
            out = whisper_mcp._attach_speaker_names(body, str(asset))
        self.assertEqual(out["words"][0]["speaker_name"], "Tadiwa")

    def test_malformed_map_is_ignored(self) -> None:
        body = _diarized_body()
        with tempfile.TemporaryDirectory() as tmp:
            asset = Path(tmp) / "clip.mp4"
            asset.write_bytes(b"")
            (Path(tmp) / "speaker_names.json").write_text("not json", encoding="utf-8")
            out = whisper_mcp._attach_speaker_names(body, str(asset))
        for w in out["words"]:
            self.assertIsNone(w["speaker_name"])


if __name__ == "__main__":
    unittest.main()
