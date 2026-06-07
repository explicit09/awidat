"""Speech-to-text + diarization indexer for Montage using WhisperX.

Produces:
- `words[]` — every word with `text`, `start_s`, `end_s`, `speaker_id`,
  and `speaker_name` (when diarization runs).
- `segments[]` — sentence-level groupings with the same fields.
- `speakers[]` — derived list of `{id, name, total_speech_s}`.

Speaker name mapping (opt-in, additive)
----------------------------------------
Diarization labels are anonymous (`SPEAKER_00`, `Speaker 0`, …). To attach
real names, drop a `speaker_names.json` next to the asset or in an ancestor
directory (or point `WHISPER_SPEAKER_NAMES` at an explicit path). When found,
each word/segment gains a `speaker_name` field and `speakers[]` entries gain a
`name`. The field is `null` for any speaker the map doesn't cover, and the
existing schema is otherwise unchanged.

Three map shapes are supported (mix freely in one file):

1. Direct id → name:
   `{"SPEAKER_00": "Tadiwa", "SPEAKER_01": "Elvis"}`
   (Deepgram ids look like `"Speaker 0"`.)

2. By total-speech-time rank — when you don't know which anonymous id is
   which, label by how much each person talks (most talk-time first):
   `{"@by_rank": ["Tadiwa", "Elvis"]}`  → primary speaker = Tadiwa.

3. By first-to-speak order:
   `{"@by_order": ["Host", "Guest"]}`  → whoever speaks first = Host.

Direct ids win over `@by_rank`, which wins over `@by_order`, so you can pin one
known speaker by id and let ranking fill in the rest.

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
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

from montage_mcp import IndexAssetRequest, IndexerServer

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
DEEPGRAM_API_KEY = os.environ.get("DEEPGRAM_API_KEY")
DEEPGRAM_MODEL = os.environ.get("DEEPGRAM_MODEL", "nova-3")
DEEPGRAM_LANGUAGE = os.environ.get("DEEPGRAM_LANGUAGE") or WHISPER_LANGUAGE or "multi"
DEEPGRAM_TIMEOUT_S = float(os.environ.get("DEEPGRAM_TIMEOUT_S", "1800"))
BACKEND = os.environ.get(
    "WHISPER_BACKEND",
    "deepgram" if DEEPGRAM_API_KEY else "auto",
).lower()
WHISPER_CPP_MODEL = os.environ.get(
    "WHISPER_CPP_MODEL",
    str(Path.home() / ".cache/montage/whisper.cpp/ggml-large-v3-turbo-q5_0.bin"),
)
WHISPER_CPP_CHUNK_S = float(os.environ.get("WHISPER_CPP_CHUNK_S", "30"))
WHISPER_CPP_OVERLAP_S = float(os.environ.get("WHISPER_CPP_OVERLAP_S", "2"))
WHISPER_CPP_THREADS = os.environ.get("WHISPER_CPP_THREADS")
WHISPER_CPP_PROCESSORS = os.environ.get("WHISPER_CPP_PROCESSORS")
WHISPER_CPP_DEVICE = os.environ.get("WHISPER_CPP_DEVICE")
WHISPER_CPP_NO_GPU = os.environ.get("WHISPER_CPP_NO_GPU", "").lower() in (
    "1",
    "true",
    "yes",
)
WHISPER_CPP_EXTRA_ARGS = os.environ.get("WHISPER_CPP_EXTRA_ARGS", "")
MAX_WORD_DUR_S = float(os.environ.get("WHISPER_MAX_WORD_DUR_S", "3"))
MAX_SEGMENT_DUR_S = float(os.environ.get("WHISPER_MAX_SEGMENT_DUR_S", "30"))
# Optional explicit path to a speaker-name map; otherwise we look for
# `speaker_names.json` beside the asset and up its ancestor directories.
SPEAKER_NAMES_PATH = os.environ.get("WHISPER_SPEAKER_NAMES")
SPEAKER_NAMES_FILENAME = "speaker_names.json"
# Reserved keys in speaker_names.json that carry ordering hints instead of a
# direct id → name mapping.
SPEAKER_NAMES_RANK_KEY = "@by_rank"
SPEAKER_NAMES_ORDER_KEY = "@by_order"


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
    return _attach_speaker_names(_transcribe(req), req.asset_path)


def _transcribe(req: IndexAssetRequest) -> dict[str, Any]:
    if BACKEND in ("deepgram", "deepgram-cloud", "cloud"):
        return _handle_deepgram(req)
    if BACKEND == "whisperx":
        return _handle_whisperx(req)
    if BACKEND in ("auto", "whispercpp-aligned", "whispercpp_aligned"):
        if DEEPGRAM_API_KEY:
            try:
                return _handle_deepgram(req)
            except Exception as e:  # noqa: BLE001
                print(
                    f"whisper-mcp: Deepgram backend failed ({e}); "
                    "falling back to local backend",
                    file=sys.stderr,
                )
        if _can_use_whispercpp_backend():
            try:
                return _handle_whispercpp_aligned(req)
            except Exception as e:  # noqa: BLE001
                if BACKEND != "auto":
                    raise
                print(
                    f"whisper-mcp: whisper.cpp aligned backend failed ({e}); "
                    "falling back to WhisperX",
                    file=sys.stderr,
                )
        elif BACKEND != "auto":
            raise RuntimeError(
                "whisper.cpp aligned backend requested but whisper-cli, ffmpeg, "
                f"ffprobe, or model is missing; WHISPER_CPP_MODEL={WHISPER_CPP_MODEL}"
            )
    return _handle_whisperx(req)


def _handle_deepgram(req: IndexAssetRequest) -> dict[str, Any]:
    if not DEEPGRAM_API_KEY:
        raise RuntimeError("WHISPER_BACKEND=deepgram requires DEEPGRAM_API_KEY")
    if not shutil.which("ffmpeg"):
        raise RuntimeError("WHISPER_BACKEND=deepgram requires ffmpeg for audio extraction")

    asset = Path(req.asset_path)
    print(
        f"whisper-mcp: using Deepgram backend model={DEEPGRAM_MODEL}",
        file=sys.stderr,
    )
    with tempfile.TemporaryDirectory(prefix="montage-deepgram-") as tmp:
        audio_path = Path(tmp) / "audio.m4a"
        _extract_upload_audio(asset, audio_path)
        response = _post_deepgram(audio_path)

    body = _convert_deepgram_response(response)
    _validate_editor_timestamps(body["segments"], body["words"])
    return body


def _extract_upload_audio(asset: Path, output: Path) -> None:
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
            "-b:a",
            "48k",
            str(output),
        ],
        check=True,
    )


def _post_deepgram(audio_path: Path) -> dict[str, Any]:
    params = {
        "model": DEEPGRAM_MODEL,
        "language": DEEPGRAM_LANGUAGE,
        "smart_format": "true",
        "punctuate": "true",
        "utterances": "true",
        "words": "true",
        "diarize": "true" if DIARIZE else "false",
    }
    url = "https://api.deepgram.com/v1/listen?" + urllib.parse.urlencode(params)
    data = audio_path.read_bytes()
    req = urllib.request.Request(
        url,
        data=data,
        method="POST",
        headers={
            "Authorization": f"Token {DEEPGRAM_API_KEY}",
            "Content-Type": "audio/*",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=DEEPGRAM_TIMEOUT_S) as resp:
            return json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"Deepgram error ({e.code}): {body}") from e


def _convert_deepgram_response(response: dict[str, Any]) -> dict[str, Any]:
    results = response.get("results") or {}
    channels = results.get("channels") or []
    if not channels:
        return _empty_transcript_body("deepgram")
    alternatives = channels[0].get("alternatives") or []
    if not alternatives:
        return _empty_transcript_body("deepgram")

    alternative = alternatives[0]
    dg_words = alternative.get("words") or []
    words: list[dict[str, Any]] = []
    speaker_durations: dict[str, float] = {}
    for word in dg_words:
        if not isinstance(word, dict):
            continue
        text = str(word.get("punctuated_word") or word.get("word") or "").strip()
        if not text or not re.search(r"[A-Za-z0-9]", text):
            continue
        start = word.get("start")
        end = word.get("end")
        if start is None or end is None:
            continue
        speaker = _deepgram_speaker_id(word.get("speaker"))
        start_f = float(start)
        end_f = float(end)
        item = {
            "text": text,
            "start_s": start_f,
            "end_s": end_f,
            "speaker_id": speaker,
        }
        if word.get("confidence") is not None:
            item["confidence"] = float(word["confidence"])
        words.append(item)
        if speaker:
            speaker_durations[speaker] = speaker_durations.get(speaker, 0.0) + (
                end_f - start_f
            )

    words = _repair_words_for_editor(words)
    segments = _repair_segments_for_editor(_deepgram_segments(results.get("utterances"), words))
    speakers = [
        {"id": sid, "total_speech_s": round(dur, 3)}
        for sid, dur in sorted(speaker_durations.items())
    ]
    metadata = response.get("metadata") or {}
    return {
        "language": DEEPGRAM_LANGUAGE,
        "detected_language": _deepgram_detected_language(channels[0]),
        "model": f"deepgram:{DEEPGRAM_MODEL}",
        "provider": "deepgram",
        "duration_s": metadata.get("duration"),
        "words": words,
        "segments": segments,
        "speakers": speakers,
        "diarized": bool(DIARIZE and speakers),
    }


def _empty_transcript_body(model_name: str) -> dict[str, Any]:
    return {
        "language": DEEPGRAM_LANGUAGE,
        "model": model_name,
        "words": [],
        "segments": [],
        "speakers": [],
        "diarized": False,
    }


def _deepgram_detected_language(channel: dict[str, Any]) -> str | None:
    value = channel.get("detected_language")
    return str(value) if value else None


def _deepgram_speaker_id(value: Any) -> str | None:
    if value is None:
        return None
    return f"Speaker {value}"


def _deepgram_segments(
    utterances: Any,
    words: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    segments: list[dict[str, Any]] = []
    if isinstance(utterances, list):
        for utt in utterances:
            if not isinstance(utt, dict):
                continue
            text = str(utt.get("transcript") or "").strip()
            start = utt.get("start")
            end = utt.get("end")
            if not text or start is None or end is None:
                continue
            speaker = _deepgram_speaker_id(utt.get("speaker"))
            segments.append(
                {
                    "text": text,
                    "start_s": float(start),
                    "end_s": float(end),
                    "speaker_id": speaker,
                }
            )
        if segments:
            return segments

    return _segments_from_words(words)


def _segments_from_words(words: list[dict[str, Any]]) -> list[dict[str, Any]]:
    segments: list[dict[str, Any]] = []
    cur: list[dict[str, Any]] = []
    cur_speaker: str | None = None
    for word in words:
        speaker = word.get("speaker_id")
        gap = 0.0 if not cur else float(word["start_s"]) - float(cur[-1]["end_s"])
        duration = 0.0 if not cur else float(word["end_s"]) - float(cur[0]["start_s"])
        text = str(word["text"])
        should_break = bool(
            cur
            and (
                gap > 0.75
                or duration > MAX_SEGMENT_DUR_S
                or speaker != cur_speaker
                or str(cur[-1]["text"]).endswith((".", "?", "!"))
            )
        )
        if should_break:
            segments.append(_segment_from_word_group(cur, cur_speaker))
            cur = []
        cur.append(word)
        cur_speaker = speaker
        if text.endswith((".", "?", "!")):
            segments.append(_segment_from_word_group(cur, cur_speaker))
            cur = []
            cur_speaker = None
    if cur:
        segments.append(_segment_from_word_group(cur, cur_speaker))
    return segments


def _repair_words_for_editor(words: list[dict[str, Any]]) -> list[dict[str, Any]]:
    repaired: list[dict[str, Any]] = []
    prev_end = 0.0
    for word in sorted(
        words,
        key=lambda item: (float(item.get("start_s", 0.0)), float(item.get("end_s", 0.0))),
    ):
        text = str(word.get("text") or "").strip()
        if not text:
            continue
        start = max(0.0, float(word.get("start_s", 0.0)))
        end = max(start + 0.04, float(word.get("end_s", start + 0.04)))
        if start < prev_end:
            start = prev_end
            end = max(end, start + 0.04)
        if end - start > MAX_WORD_DUR_S:
            end = start + MAX_WORD_DUR_S
        fixed = dict(word)
        fixed["text"] = text
        fixed["start_s"] = round(start, 3)
        fixed["end_s"] = round(end, 3)
        repaired.append(fixed)
        prev_end = fixed["end_s"]
    return repaired


def _repair_segments_for_editor(segments: list[dict[str, Any]]) -> list[dict[str, Any]]:
    repaired: list[dict[str, Any]] = []
    for segment in sorted(
        segments,
        key=lambda item: (float(item.get("start_s", 0.0)), float(item.get("end_s", 0.0))),
    ):
        start = max(0.0, float(segment.get("start_s", 0.0)))
        end = max(start + 0.04, float(segment.get("end_s", start + 0.04)))
        if end - start > MAX_SEGMENT_DUR_S:
            end = start + MAX_SEGMENT_DUR_S
        fixed = dict(segment)
        fixed["start_s"] = round(start, 3)
        fixed["end_s"] = round(end, 3)
        repaired.append(fixed)
    return repaired


def _segment_from_word_group(
    words: list[dict[str, Any]],
    speaker: str | None,
) -> dict[str, Any]:
    return {
        "text": " ".join(str(w["text"]) for w in words),
        "start_s": float(words[0]["start_s"]),
        "end_s": float(words[-1]["end_s"]),
        "speaker_id": speaker,
    }


def _handle_whisperx(req: IndexAssetRequest) -> dict[str, Any]:
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
            if not text or not re.search(r"[A-Za-z0-9]", text):
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


def _can_use_whispercpp_backend() -> bool:
    return bool(
        shutil.which("whisper-cli")
        and shutil.which("ffmpeg")
        and shutil.which("ffprobe")
        and Path(WHISPER_CPP_MODEL).expanduser().is_file()
    )


def _handle_whispercpp_aligned(req: IndexAssetRequest) -> dict[str, Any]:
    import whisperx  # type: ignore[import-not-found]
    import whisperx.diarize  # type: ignore[import-not-found]  # noqa: F401

    asset = Path(req.asset_path)
    language = WHISPER_LANGUAGE or "en"
    model_path = Path(WHISPER_CPP_MODEL).expanduser()
    device = _device_and_compute_type()[0]

    print(
        f"whisper-mcp: using whisper.cpp aligned backend model={model_path}",
        file=sys.stderr,
    )
    with tempfile.TemporaryDirectory(prefix="montage-whispercpp-") as tmp:
        work_dir = Path(tmp)
        duration_s = _media_duration_s(asset)
        raw_segments = _run_whispercpp_chunks(
            asset=asset,
            model_path=model_path,
            work_dir=work_dir,
            duration_s=duration_s,
            language=language,
        )

    if not raw_segments:
        raise RuntimeError("whisper.cpp produced no transcript segments")

    with _silence_stdout():
        audio = whisperx.load_audio(str(asset))
        align_model, align_meta = whisperx.load_align_model(
            language_code=language,
            device=device,
        )
        result = whisperx.align(
            raw_segments,
            align_model,
            align_meta,
            audio,
            device,
            return_char_alignments=False,
        )

    speakers_used = False
    if DIARIZE and HF_TOKEN:
        try:
            with _silence_stdout():
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

    _repair_long_words(result, MAX_WORD_DUR_S)
    body = _flatten_result(result, language, f"whisper.cpp:{model_path.name}", speakers_used)
    _validate_editor_timestamps(body["segments"], body["words"])
    return body


def _media_duration_s(asset: Path) -> float:
    proc = subprocess.run(
        [
            "ffprobe",
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            str(asset),
        ],
        check=True,
        text=True,
        capture_output=True,
    )
    return float(proc.stdout.strip())


def _run_whispercpp_chunks(
    *,
    asset: Path,
    model_path: Path,
    work_dir: Path,
    duration_s: float,
    language: str,
) -> list[dict[str, Any]]:
    segments: list[dict[str, Any]] = []
    step_s = WHISPER_CPP_CHUNK_S - WHISPER_CPP_OVERLAP_S
    if step_s <= 0:
        raise RuntimeError("WHISPER_CPP_CHUNK_S must be greater than WHISPER_CPP_OVERLAP_S")

    start_s = 0.0
    idx = 0
    while start_s < duration_s:
        chunk_duration = min(WHISPER_CPP_CHUNK_S, duration_s - start_s)
        chunk_wav = work_dir / f"chunk-{idx:04d}.wav"
        _extract_chunk(asset, chunk_wav, start_s, chunk_duration)
        output_stem = work_dir / f"chunk-{idx:04d}"
        cmd = [
            "whisper-cli",
            "-m",
            str(model_path),
            "-f",
            str(chunk_wav),
            "-l",
            language,
            "-ojf",
            "-of",
            str(output_stem),
            "-np",
        ]
        cmd.extend(_whispercpp_hardware_args())
        proc = subprocess.run(cmd, text=True, capture_output=True, check=False)
        if proc.returncode != 0:
            raise RuntimeError(f"whisper-cli failed on chunk {idx}: {proc.stderr[-1000:]}")

        data = json.loads(output_stem.with_suffix(".json").read_text())
        keep_start = start_s if idx == 0 else start_s + WHISPER_CPP_OVERLAP_S / 2
        keep_end = start_s + chunk_duration
        if start_s + chunk_duration < duration_s:
            keep_end -= WHISPER_CPP_OVERLAP_S / 2
        for seg in data.get("transcription", []):
            parsed = _parse_whispercpp_segment(seg, start_s)
            if parsed is None or _is_suspicious_short_long_span(parsed):
                continue
            midpoint = (parsed["start"] + parsed["end"]) / 2
            if keep_start <= midpoint < keep_end:
                segments.append(parsed)

        idx += 1
        start_s += step_s
    return segments


def _whispercpp_hardware_args() -> list[str]:
    args: list[str] = []
    if WHISPER_CPP_THREADS:
        args.extend(["-t", WHISPER_CPP_THREADS])
    if WHISPER_CPP_PROCESSORS:
        args.extend(["-p", WHISPER_CPP_PROCESSORS])
    if WHISPER_CPP_DEVICE:
        args.extend(["--device", WHISPER_CPP_DEVICE])
    if WHISPER_CPP_NO_GPU:
        args.append("-ng")
    if WHISPER_CPP_EXTRA_ARGS:
        args.extend(shlex.split(WHISPER_CPP_EXTRA_ARGS))
    return args


def _extract_chunk(asset: Path, output: Path, start_s: float, duration_s: float) -> None:
    subprocess.run(
        [
            "ffmpeg",
            "-nostdin",
            "-loglevel",
            "error",
            "-y",
            "-ss",
            f"{start_s:.3f}",
            "-i",
            str(asset),
            "-t",
            f"{duration_s:.3f}",
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            str(output),
        ],
        check=True,
    )


def _parse_whispercpp_segment(seg: Any, offset_s: float) -> dict[str, Any] | None:
    if not isinstance(seg, dict):
        return None
    offsets = seg.get("offsets")
    if not isinstance(offsets, dict):
        return None
    text = _normalize_segment_text(str(seg.get("text", "")))
    if not text:
        return None
    start = offset_s + float(offsets.get("from", 0)) / 1000
    end = offset_s + float(offsets.get("to", 0)) / 1000
    if end <= start:
        return None
    return {"start": start, "end": end, "text": text}


def _normalize_segment_text(text: str) -> str:
    text = re.sub(r"<\|[^|]+\|>", " ", text)
    text = text.replace("[BLANK_AUDIO]", " ")
    return re.sub(r"\s+", " ", text).strip()


def _is_suspicious_short_long_span(seg: dict[str, Any]) -> bool:
    duration = float(seg["end"]) - float(seg["start"])
    word_count = len(re.findall(r"[a-z0-9']+", str(seg.get("text", "")).lower()))
    return duration > 15 and word_count <= 10


def _repair_long_words(result: dict[str, Any], max_word_s: float) -> None:
    for seg in result.get("segments", []):
        words = seg.get("words", [])
        if not isinstance(words, list):
            continue
        for word in words:
            if "start" not in word or "end" not in word:
                continue
            start = float(word["start"])
            end = float(word["end"])
            if end <= start:
                word["end"] = round(start + 0.04, 3)
            elif end - start > max_word_s:
                word["end"] = round(start + max_word_s, 3)
        timed = [w for w in words if isinstance(w, dict) and "start" in w and "end" in w]
        if timed:
            seg["start"] = min(float(w["start"]) for w in timed)
            seg["end"] = max(float(w["end"]) for w in timed)


def _validate_editor_timestamps(
    segments: list[dict[str, Any]],
    words: list[dict[str, Any]],
) -> None:
    for seg in segments:
        start = float(seg["start_s"])
        end = float(seg["end_s"])
        if end <= start or end - start > MAX_SEGMENT_DUR_S:
            raise RuntimeError(f"unsafe segment timing: {start:.3f}-{end:.3f}")
    prev_end = 0.0
    for word in words:
        start = float(word["start_s"])
        end = float(word["end_s"])
        if end <= start or end - start > MAX_WORD_DUR_S:
            raise RuntimeError(f"unsafe word timing: {word['text']} {start:.3f}-{end:.3f}")
        if start < prev_end - 0.1:
            raise RuntimeError(f"non-monotonic word timing near {word['text']}")
        prev_end = max(prev_end, end)


def _flatten_result(
    result: dict[str, Any],
    language: str,
    model_name: str,
    speakers_used: bool,
) -> dict[str, Any]:
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
            if not text or not re.search(r"[A-Za-z0-9]", text):
                continue
            start = w.get("start")
            end = w.get("end")
            if start is None or end is None:
                continue
            speaker = w.get("speaker") or seg_speaker
            start_f = float(start)
            end_f = float(end)
            words.append(
                {
                    "text": text,
                    "start_s": start_f,
                    "end_s": end_f,
                    "speaker_id": speaker,
                }
            )
            if speaker:
                speaker_durations[speaker] = speaker_durations.get(speaker, 0.0) + (
                    end_f - start_f
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


# ---------------------------------------------------------------------------
# Speaker name mapping (opt-in, additive). See module docstring for the JSON
# shapes. These helpers are pure (no ML deps) so every backend can call them.
# ---------------------------------------------------------------------------


def _find_speaker_names_file(asset_path: str) -> Path | None:
    """Locate a `speaker_names.json` for this asset, or None.

    Resolution order:
    1. `WHISPER_SPEAKER_NAMES` env var (explicit file path).
    2. `speaker_names.json` beside the asset, then each ancestor directory
       (lets one map cover a whole project tree).
    """
    if SPEAKER_NAMES_PATH:
        explicit = Path(SPEAKER_NAMES_PATH).expanduser()
        return explicit if explicit.is_file() else None

    asset = Path(asset_path).expanduser()
    for directory in [asset.parent, *asset.parent.parents]:
        candidate = directory / SPEAKER_NAMES_FILENAME
        if candidate.is_file():
            return candidate
    return None


def _load_speaker_name_map(asset_path: str) -> dict[str, Any]:
    """Read the raw speaker-name map for this asset ({} when absent/invalid)."""
    path = _find_speaker_names_file(asset_path)
    if path is None:
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        print(
            f"whisper-mcp: ignoring {path} — unreadable speaker name map ({e})",
            file=sys.stderr,
        )
        return {}
    if not isinstance(data, dict):
        print(
            f"whisper-mcp: ignoring {path} — expected a JSON object",
            file=sys.stderr,
        )
        return {}
    return data


def _resolve_speaker_names(
    raw_map: dict[str, Any],
    speakers: list[dict[str, Any]],
    first_seen_order: list[str],
) -> dict[str, str]:
    """Build a concrete `speaker_id -> name` map from raw config + signals.

    Direct id entries win over `@by_rank` (by total_speech_s, descending),
    which wins over `@by_order` (first-to-speak). `speakers` carries the
    `total_speech_s` used for ranking; `first_seen_order` is ids in the order
    they first appear in the transcript.
    """
    if not raw_map:
        return {}

    resolved: dict[str, str] = {}

    def _assign(ids: list[str], names: Any) -> None:
        if not isinstance(names, list):
            return
        for sid, name in zip(ids, names):
            if sid in resolved:
                continue
            if isinstance(name, str) and name.strip():
                resolved[sid] = name.strip()

    # First writer wins, so apply in descending precedence: direct ids, then
    # @by_rank (total_speech_s desc), then @by_order (first-to-speak). Applying
    # order first would invert the documented precedence and let order shadow
    # a rank mapping for an already-seen speaker.
    for key, value in raw_map.items():
        if key in (SPEAKER_NAMES_RANK_KEY, SPEAKER_NAMES_ORDER_KEY):
            continue
        if isinstance(value, str) and value.strip():
            resolved[key] = value.strip()

    rank_ids = [
        s["id"]
        for s in sorted(
            speakers,
            key=lambda s: (-float(s.get("total_speech_s", 0.0)), str(s.get("id"))),
        )
    ]
    _assign(rank_ids, raw_map.get(SPEAKER_NAMES_RANK_KEY))

    order_ids = list(first_seen_order)
    _assign(order_ids, raw_map.get(SPEAKER_NAMES_ORDER_KEY))

    return resolved


def _attach_speaker_names(body: dict[str, Any], asset_path: str) -> dict[str, Any]:
    """Add `speaker_name`/`name` fields to a transcript body in place.

    Always runs (additive + null-safe): `speaker_name` is set on every word and
    segment, and `name` on every `speakers[]` entry. When no map applies the
    values are null, preserving the existing schema for unmapped speakers.
    """
    words = body.get("words") or []
    segments = body.get("segments") or []
    speakers = body.get("speakers") or []

    first_seen_order: list[str] = []
    seen: set[str] = set()
    for word in words:
        sid = word.get("speaker_id")
        if sid and sid not in seen:
            seen.add(sid)
            first_seen_order.append(sid)

    raw_map = _load_speaker_name_map(asset_path)
    name_by_id = _resolve_speaker_names(raw_map, speakers, first_seen_order)

    def _name_for(sid: Any) -> str | None:
        return name_by_id.get(sid) if isinstance(sid, str) else None

    for word in words:
        word["speaker_name"] = _name_for(word.get("speaker_id"))
    for segment in segments:
        segment["speaker_name"] = _name_for(segment.get("speaker_id"))
    for speaker in speakers:
        speaker["name"] = _name_for(speaker.get("id"))

    if name_by_id:
        print(
            f"whisper-mcp: applied speaker names {name_by_id}",
            file=sys.stderr,
        )
    return body


def main() -> None:
    server.run()
