import unittest

import numpy as np

from audio_energy_mcp import _LoadedAudio, _true_peak_dbfs


class AudioEnergyMeterTests(unittest.TestCase):
    def test_true_peak_dbfs_reports_peak_sample_level(self) -> None:
        audio = _LoadedAudio(
            samples=np.array([0.0, -0.25, 0.5, -0.125], dtype=np.float32),
            sample_rate=48_000,
        )

        self.assertAlmostEqual(_true_peak_dbfs(audio), -6.0206, places=3)

    def test_true_peak_dbfs_returns_none_for_silence(self) -> None:
        audio = _LoadedAudio(samples=np.zeros(8, dtype=np.float32), sample_rate=48_000)

        self.assertIsNone(_true_peak_dbfs(audio))


if __name__ == "__main__":
    unittest.main()
