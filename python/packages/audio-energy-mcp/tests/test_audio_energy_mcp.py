import contextlib
import io
import os
import tempfile
import textwrap
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import numpy as np

from audio_energy_mcp import (
    _LoadedAudio,
    _loudness,
    _silences,
    _true_peak_dbfs,
    _windowed_rms_db,
    handle,
)


class _FragmentedPipe:
    def __init__(self, fragments: list[bytes | BaseException]) -> None:
        self._fragments = iter(fragments)
        self.closed = False

    def read(self, _size: int = -1) -> bytes:
        try:
            fragment = next(self._fragments)
        except StopIteration:
            return b""
        if isinstance(fragment, BaseException):
            raise fragment
        return fragment

    def close(self) -> None:
        self.closed = True


class _FakeFfmpeg:
    def __init__(
        self,
        stdout_fragments: list[bytes | BaseException],
        *,
        return_code: int = 0,
        stderr_fragments: list[bytes | BaseException] | None = None,
    ) -> None:
        self.stdout = _FragmentedPipe(stdout_fragments)
        self.stderr = _FragmentedPipe(stderr_fragments or [])
        self._final_return_code = return_code
        self.returncode: int | None = None
        self.terminated = False
        self.killed = False
        self.wait_calls = 0

    def poll(self) -> int | None:
        return self.returncode

    def wait(self, timeout: float | None = None) -> int:
        del timeout
        self.wait_calls += 1
        if self.returncode is None:
            self.returncode = self._final_return_code
        return self.returncode

    def terminate(self) -> None:
        self.terminated = True
        self.returncode = -15

    def kill(self) -> None:
        self.killed = True
        self.returncode = -9


def _reference_data(samples: np.ndarray) -> dict[str, object]:
    if len(samples) == 0:
        samples = np.zeros(48_000, dtype=np.float32)
    audio = _LoadedAudio(samples=samples, sample_rate=48_000)
    loudness = _loudness(audio)
    return {
        "sample_rate": audio.sample_rate,
        "duration_s": float(len(audio.samples) / audio.sample_rate),
        "window_ms": 100,
        "windows": _windowed_rms_db(audio),
        "loudness_integrated_lufs": loudness.get("integrated_lufs"),
        "true_peak_dbfs": _true_peak_dbfs(audio),
        "loudness_short_term": loudness.get("short_term", []),
        "silences": _silences(loudness),
        "silence_relative_lu": -30.0,
    }


def _mixed_samples() -> np.ndarray:
    sample_rate = 48_000
    t = np.arange(sample_rate * 12, dtype=np.float64) / sample_rate
    samples = np.empty_like(t, dtype=np.float32)
    samples[: 4 * sample_rate] = (0.2 * np.sin(2.0 * np.pi * 220.0 * t[: 4 * sample_rate])).astype(
        np.float32
    )
    samples[4 * sample_rate : 8 * sample_rate] = (
        0.002 * np.sin(2.0 * np.pi * 220.0 * t[4 * sample_rate : 8 * sample_rate])
    ).astype(np.float32)
    samples[8 * sample_rate :] = (0.2 * np.sin(2.0 * np.pi * 220.0 * t[8 * sample_rate :])).astype(
        np.float32
    )
    return samples


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

    def test_handle_matches_existing_analysis_with_non_aligned_pcm_fragments(self) -> None:
        samples = _mixed_samples()
        raw = samples.astype("<f4", copy=False).tobytes()
        fragment_sizes = [1, 3, 7, 31, 257, 4093, 17, 65_537]
        fragments: list[bytes] = []
        offset = 0
        index = 0
        while offset < len(raw):
            end = min(offset + fragment_sizes[index % len(fragment_sizes)], len(raw))
            fragments.append(raw[offset:end])
            offset = end
            index += 1
        process = _FakeFfmpeg(fragments)

        with patch("audio_energy_mcp.subprocess.Popen", return_value=process):
            actual = handle(SimpleNamespace(asset_path="fixture.wav"))

        self.assertEqual(actual, _reference_data(samples))
        self.assertTrue(actual["silences"])

    def test_handle_matches_pyloudnorm_ebu_rounding_at_partial_block_boundaries(self) -> None:
        for sample_count in [19_200, 21_600, 21_601, 23_999, 24_000]:
            with self.subTest(sample_count=sample_count):
                t = np.arange(sample_count, dtype=np.float64) / 48_000
                samples = (0.25 * np.sin(2.0 * np.pi * 440.0 * t)).astype(np.float32)
                process = _FakeFfmpeg([samples.astype("<f4", copy=False).tobytes()])

                with patch("audio_energy_mcp.subprocess.Popen", return_value=process):
                    actual = handle(SimpleNamespace(asset_path="fixture.wav"))

                self.assertEqual(actual, _reference_data(samples))

    def test_handle_emits_one_second_zero_result_after_successful_empty_decode(self) -> None:
        process = _FakeFfmpeg([])

        with (
            patch("audio_energy_mcp.subprocess.Popen", return_value=process),
            contextlib.redirect_stderr(io.StringIO()) as stderr,
        ):
            actual = handle(SimpleNamespace(asset_path="empty.wav"))

        self.assertEqual(actual, _reference_data(np.empty(0, dtype=np.float32)))
        self.assertIn("no audio stream in empty.wav", stderr.getvalue())

    def test_handle_preserves_short_input_error(self) -> None:
        samples = np.full(19_199, 0.25, dtype=np.float32)
        process = _FakeFfmpeg([samples.astype("<f4", copy=False).tobytes()])

        with (
            patch("audio_energy_mcp.subprocess.Popen", return_value=process),
            self.assertRaisesRegex(ValueError, "Audio must have length greater than the block size"),
        ):
            handle(SimpleNamespace(asset_path="short.wav"))

        self.assertEqual(process.wait_calls, 1)
        self.assertTrue(process.stdout.closed)
        self.assertTrue(process.stderr.closed)

    def test_handle_discards_partial_pcm_when_ffmpeg_fails(self) -> None:
        process = _FakeFfmpeg(
            [np.full(19_200, 0.25, dtype="<f4").tobytes()],
            return_code=13,
            stderr_fragments=[b"invalid audio stream"],
        )

        with (
            patch("audio_energy_mcp.subprocess.Popen", return_value=process),
            self.assertRaisesRegex(
                RuntimeError,
                "ffmpeg decode failed \\(exit 13\\) for broken.wav: invalid audio stream",
            ),
        ):
            handle(SimpleNamespace(asset_path="broken.wav"))

        self.assertEqual(process.wait_calls, 1)
        self.assertTrue(process.stdout.closed)
        self.assertTrue(process.stderr.closed)

    def test_handle_reaps_ffmpeg_when_stream_read_is_cancelled(self) -> None:
        process = _FakeFfmpeg([b"\\x00\\x00", KeyboardInterrupt()])

        with (
            patch("audio_energy_mcp.subprocess.Popen", return_value=process),
            self.assertRaises(KeyboardInterrupt),
        ):
            handle(SimpleNamespace(asset_path="cancelled.wav"))

        self.assertTrue(process.terminated)
        self.assertFalse(process.killed)
        self.assertEqual(process.wait_calls, 1)
        self.assertTrue(process.stdout.closed)
        self.assertTrue(process.stderr.closed)

    def test_handle_does_not_create_a_pcm_temp_file_while_decoding(self) -> None:
        """A decoder can observe whether its caller stages full PCM on disk."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            marker = root / "decoder-observation.txt"
            decoder = root / "fake-ffmpeg.py"
            decoder.write_text(
                textwrap.dedent(
                    """\
                    #!/usr/bin/env python3
                    import glob
                    import os
                    import struct
                    import sys

                    temp_files = glob.glob(
                        os.path.join(os.environ["TMPDIR"], "montage-audio-*.f32")
                    )
                    with open(os.environ["AUDIO_ENERGY_TEMP_MARKER"], "w", encoding="utf-8") as handle:
                        handle.write("pcm-temp" if temp_files else "streaming")
                    sys.stdout.buffer.write(struct.pack("<f", 0.5) * 48_000)
                    """
                ),
                encoding="utf-8",
            )
            decoder.chmod(0o755)

            with (
                patch.object(tempfile, "tempdir", str(root)),
                patch.dict(
                    os.environ,
                    {
                        "MONTAGE_FFMPEG": str(decoder),
                        "TMPDIR": str(root),
                        "AUDIO_ENERGY_TEMP_MARKER": str(marker),
                    },
                    clear=False,
                ),
            ):
                result = handle(SimpleNamespace(asset_path="fixture.wav"))

            self.assertEqual(marker.read_text(encoding="utf-8"), "streaming")
            self.assertEqual(result["duration_s"], 1.0)


if __name__ == "__main__":
    unittest.main()
