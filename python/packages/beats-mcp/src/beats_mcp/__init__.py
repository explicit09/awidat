"""Musical beat indexer for Awidat.

Produces the `beats.json` shape consumed by `skills/beat-sync-editor`.
The implementation is intentionally lightweight: ffmpeg decodes media
to mono float PCM, then a small onset detector finds beat-like peaks and
derives a local tempo from inter-beat intervals.
"""

from __future__ import annotations

import array
import math
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Iterable

try:
    from awidat_mcp import IndexAssetRequest, IndexerServer
except ModuleNotFoundError:  # Allows pure helper tests without uv sync.
    IndexAssetRequest = Any  # type: ignore[assignment]
    IndexerServer = None  # type: ignore[assignment]


INDEXER_NAME = "beats"
INDEXER_VERSION = "0.1.0"
SCHEMA_VERSION = "1"
TARGET_SR = 22_050
MIN_BPM = 60.0
MAX_BPM = 200.0


def _ffmpeg_path() -> str:
    explicit = os.environ.get("AWIDAT_FFMPEG")
    if explicit:
        return explicit
    found = shutil.which("ffmpeg")
    if found:
        return found
    raise RuntimeError(
        "ffmpeg not found on PATH; install ffmpeg or set AWIDAT_FFMPEG=/path/to/ffmpeg"
    )


def _load_mono(path: str) -> tuple[list[float], int]:
    ffmpeg = _ffmpeg_path()
    cmd = [
        ffmpeg,
        "-nostdin",
        "-loglevel",
        "error",
        "-i",
        path,
        "-vn",
        "-ac",
        "1",
        "-ar",
        str(TARGET_SR),
        "-f",
        "f32le",
        "-",
    ]
    tmp = tempfile.NamedTemporaryFile(prefix="awidat-beats-", suffix=".f32", delete=False)
    tmp_path = Path(tmp.name)
    tmp.close()
    try:
        with tmp_path.open("wb") as stdout:
            proc = subprocess.run(cmd, check=False, stdout=stdout, stderr=subprocess.PIPE)
        if proc.returncode != 0:
            stderr = proc.stderr.decode("utf-8", errors="replace").strip()
            raise RuntimeError(
                f"ffmpeg decode failed (exit {proc.returncode}) for {path}: {stderr}"
            )
        samples = array.array("f")
        with tmp_path.open("rb") as fh:
            samples.frombytes(fh.read())
        return list(samples), TARGET_SR
    finally:
        tmp_path.unlink(missing_ok=True)


def analyze_beats(samples: Iterable[float], sample_rate: int) -> dict[str, Any]:
    values = [float(sample) for sample in samples]
    if sample_rate <= 0:
        raise ValueError("sample_rate must be positive")
    if not values:
        return _empty_result(sample_rate, 0.0)

    duration_s = len(values) / sample_rate
    envelope = _onset_envelope(values, sample_rate)
    peaks = _pick_peaks(envelope, sample_rate)
    if not peaks:
        return _empty_result(sample_rate, duration_s)

    beat_times = [round(time_s, 3) for time_s, _ in peaks]
    tempo = _estimate_tempo_bpm([time_s for time_s, _ in peaks])
    beats = [
        {
            "time_s": time_s,
            "strength": round(float(strength), 6),
        }
        for time_s, strength in peaks
    ]
    return {
        "sample_rate": sample_rate,
        "duration_s": round(duration_s, 3),
        "tempo_bpm": tempo,
        "beat_times_s": beat_times,
        "beats": beats,
        "method": "onset_peak_interval_v1",
        "min_bpm": MIN_BPM,
        "max_bpm": MAX_BPM,
    }


def _empty_result(sample_rate: int, duration_s: float) -> dict[str, Any]:
    return {
        "sample_rate": sample_rate,
        "duration_s": round(duration_s, 3),
        "tempo_bpm": None,
        "beat_times_s": [],
        "beats": [],
        "method": "onset_peak_interval_v1",
        "min_bpm": MIN_BPM,
        "max_bpm": MAX_BPM,
    }


def _onset_envelope(samples: list[float], sample_rate: int) -> list[tuple[float, float]]:
    hop = max(1, int(sample_rate * 0.01))
    frame = max(hop, int(sample_rate * 0.05))
    energies: list[tuple[float, float]] = []
    for start in range(0, max(0, len(samples) - frame + 1), hop):
        window = samples[start : start + frame]
        energy = sum(abs(value) for value in window) / len(window)
        energies.append((start / sample_rate, energy))

    envelope: list[tuple[float, float]] = []
    previous = 0.0
    for time_s, energy in energies:
        onset = max(0.0, energy - previous)
        envelope.append((time_s, onset))
        previous = energy
    return envelope


def _pick_peaks(envelope: list[tuple[float, float]], sample_rate: int) -> list[tuple[float, float]]:
    strengths = [strength for _, strength in envelope if strength > 0.0]
    if not strengths:
        return []
    median = statistics.median(strengths)
    threshold = max(median * 0.75, max(strengths) * 0.25)
    min_spacing_s = 60.0 / MAX_BPM
    peaks: list[tuple[float, float]] = []

    frame_shift_s = 0.03
    for time_s, strength in envelope:
        if strength < threshold:
            continue
        # Frame energy rises as soon as the analysis window begins to include
        # the transient, so shift toward the transient inside the 50 ms frame.
        corrected_time = round(time_s + frame_shift_s, 3)
        if peaks and corrected_time - peaks[-1][0] < min_spacing_s:
            if strength >= peaks[-1][1]:
                peaks[-1] = (corrected_time, strength)
            continue
        peaks.append((corrected_time, strength))
    return peaks


def _estimate_tempo_bpm(times: list[float]) -> float | None:
    intervals = [
        later - earlier
        for earlier, later in zip(times, times[1:], strict=False)
        if later > earlier
    ]
    intervals = [
        interval
        for interval in intervals
        if 60.0 / MAX_BPM <= interval <= 60.0 / MIN_BPM
    ]
    if not intervals:
        return None
    bpm = 60.0 / statistics.median(intervals)
    return round(float(bpm), 3)


if IndexerServer is not None:
    server = IndexerServer(
        name=INDEXER_NAME,
        indexer_version=INDEXER_VERSION,
        schema_version=SCHEMA_VERSION,
    )

    @server.index_asset
    def handle(req: IndexAssetRequest) -> dict[str, Any]:
        samples, sample_rate = _load_mono(req.asset_path)
        return analyze_beats(samples, sample_rate)
else:
    server = None


def main() -> None:
    if server is None:
        print("beats-mcp requires awidat-mcp; run through the uv workspace", file=sys.stderr)
        raise SystemExit(1)
    server.run()
