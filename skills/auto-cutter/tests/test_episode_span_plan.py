import json
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "episode_span_plan.py"


def segment(start, end, text):
    return {"start_s": start, "end_s": end, "text": text}


class EpisodeSpanPlanTests(unittest.TestCase):
    def run_plan(self, segments, silences=None):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            transcript = root / "transcript.json"
            audio = root / "audio.json"
            topic = root / "topic.json"
            transcript.write_text(json.dumps({"data": {"segments": segments}}))
            audio.write_text(json.dumps({"data": {"silences": silences or []}}))
            topic.write_text(json.dumps({"data": {"topics": []}}))
            result = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "--transcript",
                    str(transcript),
                    "--audio-energy",
                    str(audio),
                    "--topic",
                    str(topic),
                ],
                check=True,
                capture_output=True,
                text=True,
            )
            return json.loads(result.stdout)

    def test_single_clean_episode_is_publishable_without_user_choice(self):
        content = " ".join(["semiconductor platform founders roadmap customer traction"] * 12)
        report = self.run_plan(
            [
                segment(60, 72, "Welcome to Technologia Talks with today's founder story"),
                segment(180, 220, content),
                segment(420, 460, content),
                segment(690, 730, content),
                segment(860, 872, "Thanks for listening and see you next time"),
            ],
            silences=[{"start_s": 40, "end_s": 60}],
        )

        spans = report["episode_spans"]
        self.assertEqual(len(spans), 1)
        self.assertAlmostEqual(spans[0]["start_s"], 60.0)
        self.assertAlmostEqual(spans[0]["end_s"], 872.0)
        self.assertIn("outro_language", spans[0]["reasons"])
        self.assertFalse(report["requires_user_choice"])

    def test_two_real_episodes_split_by_sustained_meta_talk(self):
        content = " ".join(["founder story market product customers"] * 10)
        report = self.run_plan(
            [
                segment(20, 35, "off camera let's plan which topics before we begin"),
                segment(60, 70, "Welcome to Technologia Talks episode one"),
                segment(120, 150, content),
                segment(420, 450, content),
                segment(780, 810, content),
                segment(900, 925, "off camera which topics are we choosing today for the next format"),
                segment(945, 970, "our podcast the edit the thumbnail upload retention"),
                segment(1000, 1010, "Welcome back to episode two"),
                segment(1080, 1110, content),
                segment(1320, 1350, content),
                segment(1600, 1630, content),
            ],
            silences=[{"start_s": 40, "end_s": 60}, {"start_s": 1760, "end_s": 1810}],
        )

        spans = report["episode_spans"]
        self.assertEqual(len(spans), 2)
        self.assertAlmostEqual(spans[0]["start_s"], 60.0)
        self.assertAlmostEqual(spans[0]["end_s"], 900.0)
        self.assertIn("meta_talk_transition", spans[0]["reasons"])
        self.assertAlmostEqual(spans[1]["start_s"], 1000.0)
        self.assertAlmostEqual(spans[1]["end_s"], 1760.0)
        self.assertIn("sustained_silence_end", spans[1]["reasons"])
        self.assertTrue(report["requires_user_choice"])

    def test_rehearsed_intro_is_rejected_and_real_episode_is_recommended(self):
        content = " ".join(["research coral data startup execution"] * 10)
        report = self.run_plan(
            [
                segment(10, 20, "Welcome to Technologia Talks practice cut one more time"),
                segment(120, 130, "what's the other one look at that camera"),
                segment(300, 310, "Welcome to Technologia Talks with Yusuf"),
                segment(360, 390, content),
                segment(650, 680, content),
                segment(920, 950, content),
                segment(1120, 1130, "Thanks for listening see you next time"),
            ],
            silences=[{"start_s": 260, "end_s": 300}],
        )

        spans = report["episode_spans"]
        rejected = report["rejected_spans"]
        self.assertEqual(len(spans), 1)
        self.assertAlmostEqual(spans[0]["start_s"], 300.0)
        self.assertEqual(report["recommended_span"]["start_s"], 300.0)
        self.assertTrue(
            any(
                "rehearsal_or_false_start_language" in span.get("rejection_reasons", [])
                for span in rejected
            )
        )

    def test_close_followed_by_post_show_chatter_ends_before_sustained_silence(self):
        content = " ".join(["ai hardware product roadmap customer deployment"] * 12)
        report = self.run_plan(
            [
                segment(80, 92, "Welcome back to Technologia Talks"),
                segment(180, 220, content),
                segment(420, 460, content),
                segment(690, 730, content),
                segment(760, 778, "that's a wrap thanks for listening until next time"),
                segment(815, 835, "off camera let's plan the thumbnail upload and description tags"),
                segment(850, 870, "our podcast retention views and clip format for our viewers"),
            ],
            silences=[{"start_s": 900, "end_s": 950}],
        )

        spans = report["episode_spans"]
        self.assertEqual(len(spans), 1)
        self.assertAlmostEqual(spans[0]["start_s"], 80.0)
        self.assertAlmostEqual(spans[0]["end_s"], 815.0)
        self.assertIn("meta_talk_transition", spans[0]["reasons"])
        self.assertLess(spans[0]["end_s"], 900.0)


if __name__ == "__main__":
    unittest.main()
