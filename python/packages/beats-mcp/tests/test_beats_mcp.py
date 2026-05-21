from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


MODULE = Path(__file__).parents[1] / "src" / "beats_mcp" / "__init__.py"
SPEC = importlib.util.spec_from_file_location("beats_mcp", MODULE)
assert SPEC is not None and SPEC.loader is not None
beats_mcp = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(beats_mcp)


class BeatsMcpTest(unittest.TestCase):
    def test_detect_beats_from_click_track(self) -> None:
        sample_rate = 1_000
        duration_s = 4.0
        samples = [0.0] * int(sample_rate * duration_s)
        for beat_s in [0.5, 1.0, 1.5, 2.0, 2.5, 3.0]:
            start = int(beat_s * sample_rate)
            samples[start : start + 20] = [1.0] * 20

        result = beats_mcp.analyze_beats(samples, sample_rate)

        self.assertEqual(result["tempo_bpm"], 120.0)
        self.assertEqual(result["beat_times_s"][:6], [0.5, 1.0, 1.5, 2.0, 2.5, 3.0])
        self.assertEqual(result["beats"][0]["time_s"], 0.5)
        self.assertGreater(result["beats"][0]["strength"], 0.0)


if __name__ == "__main__":
    unittest.main()
