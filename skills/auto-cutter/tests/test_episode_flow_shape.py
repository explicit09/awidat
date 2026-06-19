import json
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "episode_flow_shape.py"


def segment(start, end, text):
    return {"start_s": start, "end_s": end, "text": text}


class EpisodeFlowShapeTests(unittest.TestCase):
    def run_shape(self, segments):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            transcript = root / "transcript.json"
            transcript.write_text(json.dumps({"data": {"segments": segments}}))
            result = subprocess.run(
                ["python3", str(SCRIPT), "--transcript", str(transcript)],
                check=True,
                capture_output=True,
                text=True,
            )
            return json.loads(result.stdout)

    def test_flow_shape_requires_semantic_review_for_close_then_new_episode(self):
        content = " ".join(["fable anthropic mythos product market"] * 14)
        agent_loop_content = " ".join(["agent loop workflow automation review"] * 14)

        report = self.run_shape(
            [
                segment(28, 40, "Today we are talking about Fable and Anthropic"),
                segment(120, 180, content),
                segment(1500, 1560, content),
                segment(1644, 1648, "But yeah thanks for tuning in on today's episode about Fable"),
                segment(2704, 2714, "Okay which ones are we doing I guess we can do the agent loop"),
                segment(3752, 3760, "Are you ready for agent loop okay okay"),
                segment(3860, 3920, agent_loop_content),
                segment(4600, 4660, agent_loop_content),
                segment(4922, 4930, "Wait what was the topic"),
            ]
        )

        self.assertEqual(report["status"], "needs_semantic_review")
        self.assertTrue(report["blocks_timeline_edits"])
        self.assertIn("semantic_flow_review", report["required_passes"])
        self.assertGreaterEqual(len(report["candidate_boundaries"]), 3)
        self.assertIn("thanks for tuning in", report["llm_review_packet"]["transcript_excerpt"].lower())
        self.assertIn("Are you ready for agent loop", report["llm_review_packet"]["transcript_excerpt"])
        self.assertIn("episode_spans", report["llm_review_contract"]["required_fields"])


if __name__ == "__main__":
    unittest.main()
