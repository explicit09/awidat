"""Audio-energy indexer for Montage.

Reads the asset's audio track and emits two signals the agent uses for
editorial decisions:

1. **RMS over windows** — `windows[].rms_db` at fixed 100ms granularity.
   The agent uses this to find energy peaks (good cut-on moments) and
   troughs (silences worth removing).
2. **Integrated loudness (LUFS)** — `loudness_integrated`,
   `loudness_short_term[]`. EBU R128 standard. Lets the agent reason
   about loudness in a way that ports across podcasts/interviews/conf
   recordings — a -30 LU below integrated threshold is a far better
   silence-detector than a fixed dBFS threshold.
3. **Silences** — `silences[]` derived from #2 with a relative gate.

Schema version: "1".
"""

from __future__ import annotations

import math
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Any, BinaryIO, Iterator

import numpy as np
import pyloudnorm as pyln
from scipy import signal

from montage_mcp import IndexAssetRequest, IndexerServer

INDEXER_NAME = "audio-energy"
INDEXER_VERSION = "0.1.0"
SCHEMA_VERSION = "1"

# 100ms windows over a 48kHz signal = 4800 samples. Constant across
# indexer versions; if we change it, bump SCHEMA_VERSION.
WINDOW_MS = 100

# Silence definition: short-term loudness at least this many LU below
# integrated loudness counts as silence. -30 LU = quiet relative to the
# voice of the speaker, not a fixed dBFS threshold that breaks across
# sources.
SILENCE_RELATIVE_LU = -30.0

# Minimum gap (in seconds) we'd consider worth flagging as a silence.
# Shorter gaps are inter-word breaths; the agent doesn't want to cut
# those.
MIN_SILENCE_S = 0.5


server = IndexerServer(
    name=INDEXER_NAME,
    indexer_version=INDEXER_VERSION,
    schema_version=SCHEMA_VERSION,
)


@dataclass
class _LoadedAudio:
    samples: np.ndarray  # mono float32 in [-1, 1]
    sample_rate: int
    temp_path: Path | None = None


def _ffmpeg_path() -> str:
    """Locate ffmpeg. Prefer $MONTAGE_FFMPEG, fall back to PATH."""
    explicit = os.environ.get("MONTAGE_FFMPEG")
    if explicit:
        return explicit
    found = shutil.which("ffmpeg")
    if found:
        return found
    raise RuntimeError(
        "ffmpeg not found on PATH; install ffmpeg or set MONTAGE_FFMPEG=/path/to/ffmpeg"
    )


# Working sample rate for downstream pyloudnorm + RMS analysis. 48kHz is
# the cleanest mapping to EBU R128 short-term blocks (3s window =
# 144,000 samples) and matches typical podcast/interview source rates.
_TARGET_SR = 48_000
_DECODE_CHUNK_BYTES = 64 * 1024
_MAX_STDERR_DIAGNOSTIC_BYTES = 64 * 1024
_PCM_SAMPLE_BYTES = np.dtype(np.float32).itemsize
_EBU_ENERGY_BYTES = np.dtype(np.float64).itemsize
_MAX_EBU_ENERGY_MEMORY_BYTES = 1024 * 1024
_EBU_BLOCK_S = 0.400
_EBU_STEP = 1.0 - 0.75


def _ffmpeg_command(path: str) -> list[str]:
    return [
        _ffmpeg_path(),
        "-nostdin",
        "-loglevel", "error",
        "-i", path,
        "-vn",                  # discard any video stream
        "-ac", "1",             # downmix to mono
        "-ar", str(_TARGET_SR), # resample
        "-f", "f32le",          # raw 32-bit float little-endian
        "-",                    # to stdout
    ]


class _StreamingAudio:
    """Bounded mono PCM analysis with pyloudnorm-compatible calculations."""

    def __init__(self, sample_rate: int) -> None:
        self.sample_rate = sample_rate
        self._sample_count = 0
        self._remainder = bytearray()

        self._peak = 0.0
        self._has_nan = False

        self._windows: list[dict[str, float]] = []
        self._rms_samples = np.empty(0, dtype=np.float32)
        self._rms_window_count = 0

        self._short_samples = np.empty(0, dtype=np.float32)
        self._short_buffer_start = 0
        self._next_short_start = 0
        self._short_term: list[dict[str, float]] = []

        self._ebu_samples = np.empty(0, dtype=np.float32)
        self._ebu_buffer_start = 0
        self._block_count = 0
        self._energy_file: BinaryIO = tempfile.SpooledTemporaryFile(
            max_size=_MAX_EBU_ENERGY_MEMORY_BYTES,
            mode="w+b",
            prefix="montage-audio-energy-",
            suffix=".f64",
        )

        meter = pyln.Meter(sample_rate, block_size=_EBU_BLOCK_S)
        self._filter_stages = tuple(meter._filters.values())
        self._filter_states = [
            np.zeros(max(len(stage.a), len(stage.b)) - 1)
            for stage in self._filter_stages
        ]

    def consume_bytes(self, fragment: bytes) -> None:
        self._remainder.extend(fragment)
        byte_count = len(self._remainder) - len(self._remainder) % _PCM_SAMPLE_BYTES
        if byte_count == 0:
            return
        samples = np.frombuffer(
            self._remainder,
            dtype=np.dtype("<f4"),
            count=byte_count // _PCM_SAMPLE_BYTES,
        ).copy()
        del self._remainder[:byte_count]
        self.consume_samples(samples)

    def consume_samples(self, samples: np.ndarray) -> None:
        if len(samples) == 0:
            return

        samples = np.asarray(samples, dtype=np.float32)
        self._sample_count += len(samples)
        if np.isnan(samples).any():
            self._has_nan = True
        else:
            self._peak = max(self._peak, float(np.max(np.abs(samples))))

        self._append_rms(samples)
        self._append_short_term(samples)
        self._append_ebu_blocks(self._filter(samples))

    def _append_rms(self, samples: np.ndarray) -> None:
        self._rms_samples = np.concatenate((self._rms_samples, samples))
        window_samples = max(1, int(self.sample_rate * WINDOW_MS / 1000))
        while len(self._rms_samples) >= window_samples:
            window = self._rms_samples[:window_samples].reshape(1, window_samples)
            rms = np.sqrt(np.mean(np.square(window, dtype=np.float32), axis=1))
            rms_db = 20.0 * np.log10(np.maximum(rms, 1e-7))
            self._windows.append(
                {
                    "start_s": float(self._rms_window_count * WINDOW_MS / 1000.0),
                    "rms_db": float(rms_db[0]),
                }
            )
            self._rms_window_count += 1
            self._rms_samples = self._rms_samples[window_samples:].copy()

    def _append_short_term(self, samples: np.ndarray) -> None:
        self._short_samples = np.concatenate((self._short_samples, samples))
        window_samples = 3 * self.sample_rate
        step_samples = self.sample_rate
        while self._next_short_start + window_samples <= self._sample_count:
            offset = self._next_short_start - self._short_buffer_start
            block = self._short_samples[offset : offset + window_samples]
            try:
                lufs = float(
                    pyln.Meter(self.sample_rate, block_size=_EBU_BLOCK_S).integrated_loudness(
                        block
                    )
                )
            except ValueError:
                pass
            else:
                if math.isfinite(lufs):
                    self._short_term.append(
                        {"start_s": float(self._next_short_start / self.sample_rate), "lufs": lufs}
                    )
            self._next_short_start += step_samples

        drop = self._next_short_start - self._short_buffer_start
        if drop:
            self._short_samples = self._short_samples[drop:].copy()
            self._short_buffer_start += drop

    def _filter(self, samples: np.ndarray) -> np.ndarray:
        filtered = samples
        for index, stage in enumerate(self._filter_stages):
            filtered, self._filter_states[index] = signal.lfilter(
                stage.b,
                stage.a,
                filtered,
                zi=self._filter_states[index],
            )
            # pyloudnorm assigns each stage back into its input-typed copy, so
            # float32 PCM is intentionally rounded here for exact compatibility.
            filtered = filtered.astype(np.float32)
        return filtered

    def _ebu_bounds(self, block_index: int) -> tuple[int, int]:
        lower = int(_EBU_BLOCK_S * (block_index * _EBU_STEP) * self.sample_rate)
        upper = int(_EBU_BLOCK_S * (block_index * _EBU_STEP + 1) * self.sample_rate)
        return lower, upper

    def _block_energy(self, samples: np.ndarray) -> float:
        return float(
            (1.0 / (_EBU_BLOCK_S * self.sample_rate)) * np.sum(np.square(samples))
        )

    def _record_block_energy(self, samples: np.ndarray) -> None:
        energy = self._block_energy(samples)
        self._energy_file.write(struct.pack("<d", energy))
        self._block_count += 1

    def _block_energy_values(self) -> Iterator[float]:
        self._energy_file.flush()
        self._energy_file.seek(0)
        while fragment := self._energy_file.read(_DECODE_CHUNK_BYTES):
            if len(fragment) % _EBU_ENERGY_BYTES != 0:
                raise RuntimeError("corrupt EBU energy storage")
            yield from np.frombuffer(fragment, dtype=np.dtype("<f8"))

    @staticmethod
    def _energy_loudness(energy: float) -> float:
        with np.errstate(divide="ignore", invalid="ignore"):
            return float(-0.691 + 10.0 * np.log10(energy))

    def _append_ebu_blocks(self, samples: np.ndarray) -> None:
        self._ebu_samples = np.concatenate((self._ebu_samples, samples))
        while True:
            lower, upper = self._ebu_bounds(self._block_count)
            if upper > self._sample_count:
                break
            offset = lower - self._ebu_buffer_start
            self._record_block_energy(
                self._ebu_samples[offset : offset + (upper - lower)]
            )

        next_lower, _ = self._ebu_bounds(self._block_count)
        drop = next_lower - self._ebu_buffer_start
        if drop:
            self._ebu_samples = self._ebu_samples[drop:].copy()
            self._ebu_buffer_start += drop

    def _finish_ebu_blocks(self) -> None:
        duration = self._sample_count / self.sample_rate
        block_count = int(
            np.round((duration - _EBU_BLOCK_S) / (_EBU_BLOCK_S * _EBU_STEP)) + 1
        )
        while self._block_count < block_count:
            lower, upper = self._ebu_bounds(self._block_count)
            offset = lower - self._ebu_buffer_start
            self._record_block_energy(
                self._ebu_samples[offset : offset + (upper - lower)]
            )

    def _integrated_loudness(self) -> float:
        absolute_count = 0

        def absolute_gated() -> Iterator[float]:
            nonlocal absolute_count
            for energy in self._block_energy_values():
                if self._energy_loudness(energy) >= -70.0:
                    absolute_count += 1
                    yield energy

        absolute_sum = math.fsum(absolute_gated())
        if absolute_count == 0:
            return -math.inf
        mean_energy = absolute_sum / absolute_count
        relative_gate = self._energy_loudness(mean_energy) - 10.0

        relative_count = 0

        def relative_gated() -> Iterator[float]:
            nonlocal relative_count
            for energy in self._block_energy_values():
                loudness = self._energy_loudness(energy)
                if loudness > relative_gate and loudness > -70.0:
                    relative_count += 1
                    yield energy

        relative_sum = math.fsum(relative_gated())
        if relative_count == 0:
            return -math.inf
        return self._energy_loudness(relative_sum / relative_count)

    def close(self) -> None:
        self._energy_file.close()

    def result(self, path: str) -> dict[str, Any]:
        if self._remainder:
            raise RuntimeError(f"ffmpeg produced an incomplete f32le PCM frame for {path}")
        if self._sample_count == 0:
            print(
                f"audio-energy: no audio stream in {path}; emitting empty result",
                file=sys.stderr,
            )
            self.consume_samples(np.zeros(self.sample_rate, dtype=np.float32))

        if self._sample_count < _EBU_BLOCK_S * self.sample_rate:
            raise ValueError("Audio must have length greater than the block size.")

        self._finish_ebu_blocks()
        integrated = self._integrated_loudness()
        loudness = (
            {"integrated_lufs": None, "short_term": []}
            if not math.isfinite(integrated)
            else {"integrated_lufs": integrated, "short_term": self._short_term}
        )
        peak = None
        if not self._has_nan and self._peak > 0.0 and math.isfinite(self._peak):
            peak = float(20.0 * math.log10(self._peak))
        silences = _silences(loudness)
        return {
            "sample_rate": self.sample_rate,
            "duration_s": float(self._sample_count / self.sample_rate),
            "window_ms": WINDOW_MS,
            "windows": self._windows,
            "loudness_integrated_lufs": loudness.get("integrated_lufs"),
            "true_peak_dbfs": peak,
            "loudness_short_term": loudness.get("short_term", []),
            "silences": silences,
            "silence_relative_lu": SILENCE_RELATIVE_LU,
        }


def _terminate_ffmpeg(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is None:
        try:
            process.terminate()
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            try:
                process.kill()
            except ProcessLookupError:
                pass
            process.wait()


def _retain_stderr_tail(tail: bytearray, fragment: bytes) -> None:
    if len(fragment) >= _MAX_STDERR_DIAGNOSTIC_BYTES:
        tail[:] = fragment[-_MAX_STDERR_DIAGNOSTIC_BYTES:]
        return
    overflow = len(tail) + len(fragment) - _MAX_STDERR_DIAGNOSTIC_BYTES
    if overflow > 0:
        del tail[:overflow]
    tail.extend(fragment)


def _stream_mono(path: str) -> dict[str, Any]:
    """Decode and analyze mono f32le PCM without staging decoded audio."""
    analysis = _StreamingAudio(_TARGET_SR)
    try:
        process = subprocess.Popen(
            _ffmpeg_command(path),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        if process.stdout is None or process.stderr is None:
            _terminate_ffmpeg(process)
            raise RuntimeError("ffmpeg decode did not provide stdout and stderr pipes")

        stderr_tail = bytearray()
        stderr_errors: list[BaseException] = []

        def drain_stderr() -> None:
            try:
                while fragment := process.stderr.read(_DECODE_CHUNK_BYTES):
                    _retain_stderr_tail(stderr_tail, fragment)
            except BaseException as error:
                stderr_errors.append(error)
                _terminate_ffmpeg(process)

        stderr_thread = threading.Thread(target=drain_stderr, daemon=True)
        stderr_thread.start()
        try:
            while fragment := process.stdout.read(_DECODE_CHUNK_BYTES):
                analysis.consume_bytes(fragment)
            return_code = process.wait()
        except BaseException:
            _terminate_ffmpeg(process)
            raise
        finally:
            _terminate_ffmpeg(process)
            stderr_thread.join()
            process.stdout.close()
            process.stderr.close()

        if stderr_errors:
            raise stderr_errors[0]
        if return_code != 0:
            stderr = bytes(stderr_tail).decode("utf-8", errors="replace").strip()
            raise RuntimeError(f"ffmpeg decode failed (exit {return_code}) for {path}: {stderr}")
        return analysis.result(path)
    finally:
        analysis.close()


def _windowed_rms_db(audio: _LoadedAudio) -> list[dict[str, float]]:
    win = max(1, int(audio.sample_rate * WINDOW_MS / 1000))
    n_full = len(audio.samples) // win
    if n_full == 0:
        return []
    out = []
    # Keep the working block bounded. The previous full-array
    # float64 square doubled RAM for long recordings.
    chunk_windows = 4096
    floor = 1e-7
    for start_win in range(0, n_full, chunk_windows):
        end_win = min(start_win + chunk_windows, n_full)
        start_sample = start_win * win
        end_sample = end_win * win
        block = audio.samples[start_sample:end_sample].reshape(end_win - start_win, win)
        rms = np.sqrt(np.mean(np.square(block, dtype=np.float32), axis=1))
        rms_db = 20.0 * np.log10(np.maximum(rms, floor))
        for offset, db in enumerate(rms_db):
            start_s = (start_win + offset) * WINDOW_MS / 1000.0
            out.append({"start_s": float(start_s), "rms_db": float(db)})
    return out


def _loudness(audio: _LoadedAudio) -> dict[str, Any]:
    meter = pyln.Meter(audio.sample_rate, block_size=0.400)
    integrated = float(meter.integrated_loudness(audio.samples))
    if math.isinf(integrated) or math.isnan(integrated):
        # Silent file; skip short-term to avoid pyloudnorm internal NaNs.
        return {"integrated_lufs": None, "short_term": []}

    # Short-term loudness: 3-second sliding window per ITU-R BS.1770-4.
    # Step 1 second to keep the array small.
    sr = audio.sample_rate
    win = 3 * sr
    step = 1 * sr
    samples = audio.samples
    short_term = []
    n = len(samples)
    if n >= win:
        for start in range(0, n - win + 1, step):
            block = samples[start : start + win]
            try:
                lufs = float(meter.integrated_loudness(block))
            except ValueError:
                continue
            if not math.isfinite(lufs):
                continue
            short_term.append({"start_s": float(start / sr), "lufs": lufs})
    return {"integrated_lufs": integrated, "short_term": short_term}


def _true_peak_dbfs(audio: _LoadedAudio) -> float | None:
    if len(audio.samples) == 0:
        return None
    peak = float(np.max(np.abs(audio.samples)))
    if peak <= 0.0 or not math.isfinite(peak):
        return None
    return float(20.0 * math.log10(peak))


def _silences(loudness: dict[str, Any]) -> list[dict[str, float]]:
    integrated = loudness.get("integrated_lufs")
    short_term = loudness.get("short_term", [])
    if integrated is None or not short_term:
        return []
    threshold = integrated + SILENCE_RELATIVE_LU
    silent_starts = [s["start_s"] for s in short_term if s["lufs"] < threshold]
    if not silent_starts:
        return []

    # Coalesce adjacent silent samples into runs; window step is 1s, so
    # consecutive samples differ by ~1s.
    runs: list[dict[str, float]] = []
    run_start = silent_starts[0]
    last = silent_starts[0]
    for t in silent_starts[1:]:
        if t - last <= 1.5:
            last = t
            continue
        if last - run_start >= MIN_SILENCE_S:
            runs.append({"start_s": float(run_start), "end_s": float(last + 1.0)})
        run_start = t
        last = t
    if last - run_start >= MIN_SILENCE_S:
        runs.append({"start_s": float(run_start), "end_s": float(last + 1.0)})
    return runs


@server.index_asset
def handle(req: IndexAssetRequest) -> dict[str, Any]:
    return _stream_mono(req.asset_path)


def main() -> None:
    server.run()
