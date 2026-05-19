#!/usr/bin/env python3
"""Tests for short-form render verification helpers."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))


def load_script(name: str):
    spec = importlib.util.spec_from_file_location(name, SCRIPT_DIR / f"{name}.py")
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load {name}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class CutWindowViewTests(unittest.TestCase):
    def test_build_ffmpeg_args_centers_cut_window_and_maps_single_png(self) -> None:
        cut_window_view = load_script("cut_window_view")

        args = cut_window_view.build_ffmpeg_args(
            input_path=Path("/tmp/final.mp4"),
            output_path=Path("/tmp/cut-001.png"),
            center_s=10.0,
            window_s=4.0,
            has_audio=True,
        )
        joined = " ".join(args)

        self.assertIn("trim=start=8.000:duration=4.000", joined)
        self.assertIn("atrim=start=8.000:duration=4.000", joined)
        self.assertIn("showwavespic=s=1280x160", joined)
        self.assertIn("-map [out]", joined)
        self.assertEqual(args[-1], "/tmp/cut-001.png")

    def test_render_verify_reports_planned_cut_windows(self) -> None:
        render_verify = load_script("render_verify")
        with tempfile.TemporaryDirectory() as tmp:
            report_dir = Path(tmp) / "verify"
            windows = render_verify.plan_cut_windows(
                Path("/tmp/final.mp4"),
                [1.25, 7.5],
                report_dir,
                window_s=3.0,
            )

        self.assertEqual(len(windows), 2)
        self.assertEqual(windows[0]["center_s"], 1.25)
        self.assertTrue(windows[0]["file"].endswith("cut-001-1.250s.png"))
        self.assertEqual(windows[1]["center_s"], 7.5)
        self.assertTrue(windows[1]["file"].endswith("cut-002-7.500s.png"))

    def test_verify_render_json_includes_cut_windows_when_planning_only(self) -> None:
        render_verify = load_script("render_verify")
        with tempfile.TemporaryDirectory() as tmp:
            media = Path(tmp) / "final.mp4"
            media.write_bytes(b"placeholder")
            probe_result = {
                "ok": True,
                "raw": {
                    "format": {"duration": "12.0"},
                    "streams": [{"codec_type": "video"}, {"codec_type": "audio"}],
                },
            }

            with mock.patch.object(render_verify, "probe", return_value=probe_result):
                report = render_verify.verify_render(
                    media,
                    max_duration_s=15.0,
                    cut_points_s=[4.0],
                    cut_report_dir=Path(tmp) / "verify",
                    skip_cut_window_images=True,
                )

        self.assertEqual(report["status"], "passed")
        self.assertEqual(report["checks"]["cut_windows"], "planned")
        self.assertEqual(report["review_gate"]["status"], "needs_review")
        self.assertEqual(report["review_gate"]["cut_count"], 1)
        self.assertTrue(report["review_gate"]["required_before_final_report"])
        self.assertEqual(report["cut_windows"][0]["center_s"], 4.0)
        self.assertEqual(json.loads(json.dumps(report))["status"], "passed")

    def test_verify_render_json_marks_generated_cut_windows_ready_for_review(self) -> None:
        render_verify = load_script("render_verify")
        with tempfile.TemporaryDirectory() as tmp:
            media = Path(tmp) / "final.mp4"
            media.write_bytes(b"placeholder")
            probe_result = {
                "ok": True,
                "raw": {
                    "format": {"duration": "12.0"},
                    "streams": [{"codec_type": "video"}, {"codec_type": "audio"}],
                },
            }

            with (
                mock.patch.object(render_verify, "probe", return_value=probe_result),
                mock.patch.object(render_verify, "render_cut_window", return_value={"ok": True}),
            ):
                report = render_verify.verify_render(
                    media,
                    max_duration_s=15.0,
                    cut_points_s=[4.0, 8.0],
                    cut_report_dir=Path(tmp) / "verify",
                )

        self.assertEqual(report["status"], "passed")
        self.assertEqual(report["checks"]["cut_windows"], "generated")
        self.assertEqual(report["review_gate"]["status"], "ready_for_review")
        self.assertEqual(report["review_gate"]["cut_count"], 2)
        self.assertEqual(len(report["review_gate"]["artifacts"]), 2)
        self.assertTrue(report["review_gate"]["required_before_final_report"])


if __name__ == "__main__":
    unittest.main()
