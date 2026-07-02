import os
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

from montage_mcp import IndexAssetRequest

from editorial_moments_mcp import (
    FALLBACK_MODEL,
    _fallback_moments,
    _is_auth_failure,
    handle,
)


class EditorialMomentsFallbackTests(unittest.TestCase):
    def test_auth_failure_detection_matches_provider_error_text(self) -> None:
        self.assertTrue(_is_auth_failure(RuntimeError("401 Unauthorized: invalid x-api-key")))
        self.assertTrue(_is_auth_failure(RuntimeError("authentication_error")))
        self.assertFalse(_is_auth_failure(RuntimeError("overloaded")))

    def test_fallback_generates_typed_moments_from_topics(self) -> None:
        transcript = {
            "data": {
                "segments": [
                    {
                        "start_s": 0.0,
                        "end_s": 8.0,
                        "speaker_id": "host",
                        "text": "Why does this matter for creators?",
                    },
                    {
                        "start_s": 65.0,
                        "end_s": 80.0,
                        "speaker_id": "guest",
                        "text": "The app shows how revenue moves through the workflow.",
                    },
                ]
            }
        }
        topics = [
            {"start_s": 0.0, "end_s": 30.0, "label": "opening question"},
            {"start_s": 60.0, "end_s": 120.0, "label": "product workflow"},
        ]

        moments = _fallback_moments(transcript, None, topics)

        self.assertEqual(len(moments), 2)
        self.assertEqual(moments[0].moment_id, "m_000_00")
        self.assertIn(moments[0].kind.value, {"hook", "question"})
        self.assertIn(moments[1].kind.value, {"explanation", "story"})
        self.assertIn(moments[1].broll_need.value, {"medium", "high"})
        self.assertGreaterEqual(moments[0].score, 0.5)

    def test_handle_uses_fallback_without_anthropic_key(self) -> None:
        previous_key = os.environ.pop("ANTHROPIC_API_KEY", None)
        try:
            with TemporaryDirectory() as tmp:
                root = Path(tmp)
                asset_id = "raw/a.mov"
                asset_path = root / asset_id
                asset_path.parent.mkdir(parents=True)
                asset_path.write_bytes(b"")
                whisper_path = root / "index" / "whisper" / f"{asset_id}.json"
                topic_path = root / "index" / "topic" / f"{asset_id}.json"
                whisper_path.parent.mkdir(parents=True)
                topic_path.parent.mkdir(parents=True)
                whisper_path.write_text(
                    """{
  "data": {
    "segments": [
      {"start_s": 0.0, "end_s": 10.0, "speaker_id": "host", "text": "Here is the core hook."},
      {"start_s": 70.0, "end_s": 90.0, "speaker_id": "guest", "text": "This dashboard explains the workflow."}
    ]
  }
}"""
                )
                topic_path.write_text(
                    """{
  "data": {
    "topics": [
      {"start_s": 0.0, "end_s": 30.0, "label": "intro hook"},
      {"start_s": 60.0, "end_s": 120.0, "label": "dashboard workflow"}
    ]
  }
}"""
                )

                body = handle(
                    IndexAssetRequest(
                        project_root=str(root),
                        asset_path=str(asset_path),
                        asset_id=asset_id,
                        asset_sha256="sha",
                    )
                )

                self.assertEqual(body["labeler_model"], FALLBACK_MODEL)
                self.assertEqual(body["topic_segments_processed"], 2)
                self.assertEqual(len(body["moments"]), 2)
                self.assertEqual(body["moments"][0]["moment_id"], "m_000_00")
                self.assertEqual(
                    {moment["kind"] for moment in body["moments"]},
                    {"hook", "explanation"},
                )
                self.assertIn(body["moments"][1]["broll_need"], {"medium", "high"})
        finally:
            if previous_key is not None:
                os.environ["ANTHROPIC_API_KEY"] = previous_key


if __name__ == "__main__":
    unittest.main()
