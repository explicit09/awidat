"""Audio-energy indexer for Awidat.

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
from dataclasses import dataclass
from typing import Any

import numpy as np
import pyloudnorm as pyln
import soundfile as sf

from awidat_mcp import IndexAssetRequest, IndexerServer

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


def _load_mono(path: str) -> _LoadedAudio:
    samples, sr = sf.read(path, dtype="float32", always_2d=False)
    if samples.ndim == 2:
        # Down-mix to mono.
        samples = samples.mean(axis=1)
    return _LoadedAudio(samples=samples, sample_rate=int(sr))


def _windowed_rms_db(audio: _LoadedAudio) -> list[dict[str, float]]:
    win = max(1, int(audio.sample_rate * WINDOW_MS / 1000))
    n_full = len(audio.samples) // win
    if n_full == 0:
        return []
    trimmed = audio.samples[: n_full * win].reshape(n_full, win)
    rms = np.sqrt(np.mean(trimmed.astype(np.float64) ** 2, axis=1))
    # Map zeros to a very-quiet floor so the agent doesn't see -inf.
    floor = 1e-7
    rms_db = 20.0 * np.log10(np.maximum(rms, floor))
    out = []
    for i, db in enumerate(rms_db):
        start_s = i * WINDOW_MS / 1000.0
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
    audio = _load_mono(req.asset_path)
    windows = _windowed_rms_db(audio)
    loudness = _loudness(audio)
    silences = _silences(loudness)
    return {
        "sample_rate": audio.sample_rate,
        "duration_s": float(len(audio.samples) / audio.sample_rate),
        "window_ms": WINDOW_MS,
        "windows": windows,
        "loudness_integrated_lufs": loudness.get("integrated_lufs"),
        "loudness_short_term": loudness.get("short_term", []),
        "silences": silences,
        "silence_relative_lu": SILENCE_RELATIVE_LU,
    }


def main() -> None:
    server.run()
