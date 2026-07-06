"""Parakeet + senko local backend: fast ASR with CoreML diarization.

Benchmarked 2026-07-05 on an M4/16GB against Deepgram nova-3 (full report in
`transcription-eval/results/REPORT.md` on the project drive): a 1h50m session
transcribes in ~4m45s (parakeet-mlx, 23× realtime) plus ~45s diarization
(senko, 146× realtime), versus ~2h for the WhisperX path — with accuracy in
the same band and, unlike every Whisper-family engine tested, no hallucination
or decoder collapse across long music/silence stretches.

Engines (both lazy-imported; Apple Silicon only — mlx has no other wheels):
- `parakeet-mlx` — NVIDIA Parakeet TDT 0.6b v3 on MLX. Emits subword tokens
  with native word-accurate timestamps (no wav2vec2 alignment pass needed).
- `senko` — pyannote segmentation + CAM++ embeddings compiled to CoreML.
  No HF token or gated-model acceptance required.

This module owns the parakeet-specific logic: subword→word merging, speaker
assignment from diarization turns, and orchestration. Sidecar assembly reuses
the shared helpers in `whisper_mcp` so the JSON shape stays identical across
backends.
"""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

PARAKEET_MODEL = os.environ.get(
    "WHISPER_PARAKEET_MODEL", "mlx-community/parakeet-tdt-0.6b-v3"
)
# Chunked decode bounds memory on multi-hour files; defaults match the
# parakeet-mlx CLI values the benchmark ran with.
PARAKEET_CHUNK_S = float(os.environ.get("WHISPER_PARAKEET_CHUNK_S", "120"))
PARAKEET_OVERLAP_S = float(os.environ.get("WHISPER_PARAKEET_OVERLAP_S", "15"))
# A word whose midpoint falls this close outside a diarization turn still
# gets that turn's speaker — turn boundaries jitter slightly against ASR
# word boundaries.
SPEAKER_TOLERANCE_S = 0.25


def merge_tokens_to_words(
    tokens: list[tuple[str, float, float]],
) -> list[dict[str, Any]]:
    """Merge subword pieces into words.

    Parakeet tokenization marks word starts with a leading space (the first
    piece always starts a word). Whitespace-only pieces are dropped.
    """
    words: list[dict[str, Any]] = []
    for text, start, end in tokens:
        stripped = text.strip()
        if not stripped:
            continue
        if text.startswith(" ") or not words:
            words.append(
                {
                    "text": stripped,
                    "start_s": float(start),
                    "end_s": float(end),
                    "speaker_id": None,
                }
            )
        else:
            words[-1]["text"] += stripped
            words[-1]["end_s"] = float(end)
    return words


def assign_speakers(
    words: list[dict[str, Any]],
    turns: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    """Label each word with the speaker of the turn containing its midpoint.

    Raw diarization labels are arbitrary; they map to `SPEAKER_00`,
    `SPEAKER_01`, … in order of first appearance on the timeline (the
    WhisperX convention the rest of the pipeline expects). Words whose
    midpoint lands in no turn (± tolerance) keep `speaker_id = None`.
    Returns new word dicts; the input is not mutated.
    """
    ordered = sorted(turns, key=lambda t: (float(t["start"]), float(t["end"])))
    label_map: dict[str, str] = {}
    for turn in ordered:
        raw = str(turn["speaker"])
        if raw not in label_map:
            label_map[raw] = f"SPEAKER_{len(label_map):02d}"

    def speaker_at(mid: float) -> str | None:
        for tolerance in (0.0, SPEAKER_TOLERANCE_S):
            for turn in ordered:
                if (
                    float(turn["start"]) - tolerance
                    <= mid
                    <= float(turn["end"]) + tolerance
                ):
                    return label_map[str(turn["speaker"])]
        return None

    out = []
    for word in words:
        mid = (float(word["start_s"]) + float(word["end_s"])) / 2
        out.append({**word, "speaker_id": speaker_at(mid)})
    return out


def speaker_totals(words: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Aggregate per-speaker speech time, matching the sidecar `speakers[]` shape."""
    durations: dict[str, float] = {}
    for word in words:
        speaker = word.get("speaker_id")
        if not speaker:
            continue
        durations[speaker] = durations.get(speaker, 0.0) + (
            float(word["end_s"]) - float(word["start_s"])
        )
    return [
        {"id": sid, "total_speech_s": round(dur, 3)}
        for sid, dur in sorted(durations.items())
    ]


def available() -> bool:
    """Both engines installed (Apple Silicon only) and ffmpeg present."""
    import importlib.util
    import shutil

    return bool(
        importlib.util.find_spec("parakeet_mlx")
        and importlib.util.find_spec("senko")
        and shutil.which("ffmpeg")
        and shutil.which("ffprobe")
    )


def handle(asset_path: str) -> dict[str, Any]:
    """Transcribe + diarize `asset_path` into the sidecar body shape."""
    import whisper_mcp as wm

    asset = Path(asset_path)
    print(
        f"whisper-mcp: using parakeet backend model={PARAKEET_MODEL}",
        file=sys.stderr,
    )
    duration_s = wm._media_duration_s(asset)

    with tempfile.TemporaryDirectory(prefix="montage-parakeet-") as tmp:
        # senko requires 16kHz mono 16-bit WAV; parakeet reads it happily too,
        # so both engines share one extraction.
        wav = Path(tmp) / "audio.wav"
        _extract_wav16k(asset, wav)
        words = _transcribe_words(wav)
        turns = _diarize_turns(wav) if wm.DIARIZE else []

    diarized = bool(turns)
    if diarized:
        words = assign_speakers(words, turns)

    words = wm._repair_words_for_editor(words)
    segments = wm._repair_segments_for_editor(wm._segments_from_words(words))
    body = {
        "language": wm.WHISPER_LANGUAGE or "en",
        "model": f"parakeet-mlx:{PARAKEET_MODEL.rsplit('/', 1)[-1]}",
        "duration_s": duration_s,
        "words": words,
        "segments": segments,
        "speakers": speaker_totals(words),
        "diarized": diarized,
    }
    wm._validate_editor_timestamps(body["segments"], body["words"])
    return body


def _extract_wav16k(asset: Path, output: Path) -> None:
    subprocess.run(
        [
            "ffmpeg",
            "-nostdin",
            "-loglevel",
            "error",
            "-y",
            "-i",
            str(asset),
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            str(output),
        ],
        check=True,
    )


def _transcribe_words(wav: Path) -> list[dict[str, Any]]:
    import whisper_mcp as wm

    with wm._silence_stdout():
        from parakeet_mlx import from_pretrained  # type: ignore[import-not-found]

        model = from_pretrained(PARAKEET_MODEL)
        result = model.transcribe(
            str(wav),
            chunk_duration=PARAKEET_CHUNK_S,
            overlap_duration=PARAKEET_OVERLAP_S,
        )
    tokens = [
        (tok.text, float(tok.start), float(tok.end))
        for sentence in result.sentences
        for tok in sentence.tokens
    ]
    return merge_tokens_to_words(tokens)


def _diarize_turns(wav: Path) -> list[dict[str, Any]]:
    """Run senko; on failure return [] so transcription still succeeds
    undiarized (same contract as the WhisperX backend's diarization step)."""
    import whisper_mcp as wm

    try:
        with wm._silence_stdout():
            from senko import Diarizer  # type: ignore[import-not-found]

            result = Diarizer(quiet=True).diarize(str(wav))
        return list(result["merged_segments"])
    except Exception as e:  # noqa: BLE001
        print(
            f"whisper-mcp: senko diarization skipped ({e}); "
            "transcript will have no speaker labels",
            file=sys.stderr,
        )
        return []
