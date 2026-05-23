"""Speech-to-text + diarization indexer for Awidat using WhisperX.

Produces:
- `words[]` — every word with `text`, `start_s`, `end_s`, `speaker_id`
  (when diarization runs).
- `segments[]` — sentence-level groupings with the same fields.
- `speakers[]` — derived list of `{id, total_speech_s}`.

WhisperX gives forced-alignment word timestamps (via wav2vec2), not the
interpolated ones faster-whisper emits natively. Word-accurate boundaries
are what the agent needs for "cut on a breath, not mid-word."

Diarization uses `pyannote/speaker-diarization-community-1` (CC-BY-4.0,
single click-through HF gate). If `HF_TOKEN` isn't set or the user opts
out via `WHISPER_DIARIZE=false`, transcription proceeds without speaker
labels and `speaker_id = null`.

Models default to `large-v3-turbo`. Override with `WHISPER_MODEL` env
var; `small.en` is the no-prompt fallback (~470MB vs ~1.6GB).

Schema version: "1".
"""

from __future__ import annotations

import contextlib
import os
import sys
from typing import Any

from awidat_mcp import IndexAssetRequest, IndexerServer

INDEXER_NAME = "whisper"
INDEXER_VERSION = "0.1.0"
SCHEMA_VERSION = "1"


@contextlib.contextmanager
def _silence_stdout():
    """Redirect FD 1 (stdout) to /dev/null while yielding.

    WhisperX, faster-whisper, and torch all write tqdm progress bars
    and informational messages to stdout. The MCP server uses stdout
    for its JSON-RPC channel — any pollution here makes the engine
    drop the message and report "transport closed". OS-level FD
    redirection (not `sys.stdout = …`) is what's needed because the
    underlying C extensions write directly to FD 1.

    stderr is left alone so genuine errors still reach the operator
    via the engine's collected stderr stream.
    """
    sys.stdout.flush()
    saved = os.dup(1)
    devnull = os.open(os.devnull, os.O_WRONLY)
    try:
        os.dup2(devnull, 1)
        yield
    finally:
        os.dup2(saved, 1)
        os.close(devnull)
        os.close(saved)

# Default to distil-large-v3 — drop-in replacement for large-v3 with
# ~1% WER loss and ~6× speedup on CPU. The previous default
# (large-v3-turbo) was the most accurate option but ran ~0.5×
# realtime on Apple Silicon (CTranslate2 has no MPS path), which
# meant a 1h podcast took ~2h to transcribe. distil-large-v3 runs
# ~3× realtime on the same hardware and keeps full WhisperX
# alignment compatibility — same wav2vec2 forced-alignment, same
# pyannote diarization, same JSON shape on the wire.
#
# Users on a GPU host or who specifically want the last 1% of WER
# can opt back in with `WHISPER_MODEL=large-v3-turbo`.
DEFAULT_MODEL = os.environ.get("WHISPER_MODEL", "distil-large-v3")
WHISPER_LANGUAGE = os.environ.get("WHISPER_LANGUAGE")  # None = auto-detect
DIARIZE = os.environ.get("WHISPER_DIARIZE", "true").lower() not in ("false", "0", "no")
HF_TOKEN = os.environ.get("HF_TOKEN")
DIARIZATION_MODEL = "pyannote/speaker-diarization-community-1"


server = IndexerServer(
    name=INDEXER_NAME,
    indexer_version=INDEXER_VERSION,
    schema_version=SCHEMA_VERSION,
)


def _device_and_compute_type() -> tuple[str, str]:
    try:
        import torch

        if torch.cuda.is_available():
            return "cuda", "float16"
    except Exception:  # noqa: BLE001
        pass
    # MPS (Apple Silicon) isn't supported by faster-whisper's CTranslate2
    # backend; CPU + int8 is the right macOS path.
    return "cpu", "int8"


@server.index_asset
def handle(req: IndexAssetRequest) -> dict[str, Any]:
    # Lazy imports — whisperx loads slow ML deps on first call.
    import whisperx  # type: ignore[import-not-found]
    # whisperx's top-level __init__ does NOT import the diarize
    # submodule, so `whisperx.diarize.DiarizationPipeline` fails with
    # AttributeError unless we pull the submodule in explicitly.
    import whisperx.diarize  # type: ignore[import-not-found]  # noqa: F401

    device, compute_type = _device_and_compute_type()
    model_name = DEFAULT_MODEL

    print(
        f"whisper-mcp: loading model={model_name} device={device} compute={compute_type}",
        file=sys.stderr,
    )
    # Every whisperx call below is wrapped in `_silence_stdout()` because
    # the underlying torch / tqdm / faster-whisper code prints progress
    # bars to FD 1 — which is the stdio MCP transport. Any byte that
    # leaks corrupts JSON-RPC framing and the engine reports the server
    # as crashed. See _silence_stdout docstring.
    with _silence_stdout():
        model = whisperx.load_model(model_name, device=device, compute_type=compute_type)
        audio = whisperx.load_audio(req.asset_path)
    transcribe_kwargs: dict[str, Any] = {}
    if WHISPER_LANGUAGE:
        transcribe_kwargs["language"] = WHISPER_LANGUAGE
    with _silence_stdout():
        result = model.transcribe(audio, **transcribe_kwargs)
    language = result.get("language", "en")

    # Forced word alignment (wav2vec2). This is what gives us accurate
    # word boundaries — without this WhisperX is just faster-whisper.
    try:
        with _silence_stdout():
            align_model, align_meta = whisperx.load_align_model(
                language_code=language, device=device
            )
            result = whisperx.align(
                result["segments"], align_model, align_meta, audio, device,
                return_char_alignments=False,
            )
    except Exception as e:  # noqa: BLE001
        print(f"whisper-mcp: word alignment failed ({e}); using segment-level", file=sys.stderr)

    # Diarization (optional).
    speakers_used = False
    if DIARIZE and HF_TOKEN:
        try:
            with _silence_stdout():
                # whisperx ≥3.8 renamed `use_auth_token` → `token`.
                diarize_model = whisperx.diarize.DiarizationPipeline(
                    model_name=DIARIZATION_MODEL, token=HF_TOKEN, device=device
                )
                diarize_segments = diarize_model(audio)
                result = whisperx.assign_word_speakers(diarize_segments, result)
            speakers_used = True
        except Exception as e:  # noqa: BLE001
            print(
                f"whisper-mcp: diarization skipped ({e}); set HF_TOKEN and accept "
                f"{DIARIZATION_MODEL} on huggingface.co",
                file=sys.stderr,
            )
    elif DIARIZE and not HF_TOKEN:
        print(
            "whisper-mcp: diarization disabled — set HF_TOKEN to enable speaker labels "
            f"(requires accepting {DIARIZATION_MODEL} on huggingface.co)",
            file=sys.stderr,
        )

    # Flatten into our sidecar shape.
    words: list[dict[str, Any]] = []
    segments: list[dict[str, Any]] = []
    speaker_durations: dict[str, float] = {}
    for seg in result.get("segments", []):
        seg_text = seg.get("text", "").strip()
        seg_start = float(seg.get("start", 0.0))
        seg_end = float(seg.get("end", 0.0))
        seg_speaker = seg.get("speaker")
        segments.append(
            {
                "text": seg_text,
                "start_s": seg_start,
                "end_s": seg_end,
                "speaker_id": seg_speaker,
            }
        )
        for w in seg.get("words", []):
            text = w.get("word", "").strip()
            if not text:
                continue
            start = w.get("start")
            end = w.get("end")
            if start is None or end is None:
                continue
            speaker = w.get("speaker") or seg_speaker
            words.append(
                {
                    "text": text,
                    "start_s": float(start),
                    "end_s": float(end),
                    "speaker_id": speaker,
                }
            )
            if speaker:
                speaker_durations[speaker] = (
                    speaker_durations.get(speaker, 0.0) + (float(end) - float(start))
                )

    speakers = [
        {"id": sid, "total_speech_s": round(dur, 3)}
        for sid, dur in sorted(speaker_durations.items())
    ]

    return {
        "language": language,
        "model": model_name,
        "words": words,
        "segments": segments,
        "speakers": speakers,
        "diarized": speakers_used,
    }


def main() -> None:
    server.run()
