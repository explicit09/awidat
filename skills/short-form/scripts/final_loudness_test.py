#!/usr/bin/env python3
"""Tests for final loudness helper command planning."""

from __future__ import annotations

import importlib.util
import sys
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


class FinalLoudnessTests(unittest.TestCase):
    def test_parse_loudnorm_measurement_json_from_ffmpeg_stderr(self) -> None:
        final_loudness = load_script("final_loudness")
        stderr = """
        [Parsed_loudnorm_0 @ 0x123] {
                "input_i" : "-18.34",
                "input_tp" : "-3.21",
                "input_lra" : "7.10",
                "input_thresh" : "-29.50",
                "target_offset" : "0.42"
        }
        """

        measurement = final_loudness.parse_loudnorm_measurement(stderr)

        self.assertEqual(measurement["input_i"], "-18.34")
        self.assertEqual(measurement["input_tp"], "-3.21")
        self.assertEqual(measurement["target_offset"], "0.42")

    def test_build_second_pass_args_uses_measured_loudnorm_values(self) -> None:
        final_loudness = load_script("final_loudness")
        measurement = {
            "input_i": "-18.34",
            "input_tp": "-3.21",
            "input_lra": "7.10",
            "input_thresh": "-29.50",
            "target_offset": "0.42",
        }

        args = final_loudness.build_second_pass_args(
            ffmpeg="ffmpeg",
            input_path=Path("/tmp/in.mp4"),
            output_path=Path("/tmp/out.mp4"),
            measurement=measurement,
            target_i=-14.0,
            target_tp=-1.0,
            target_lra=11.0,
        )
        joined = " ".join(args)

        self.assertIn("loudnorm=I=-14.0:TP=-1.0:LRA=11.0", joined)
        self.assertIn("measured_I=-18.34", joined)
        self.assertIn("measured_TP=-3.21", joined)
        self.assertIn("offset=0.42", joined)
        self.assertIn("-map 0:v?", joined)
        self.assertIn("-map 0:a:0", joined)


if __name__ == "__main__":
    unittest.main()
