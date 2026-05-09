#!/usr/bin/env python3
"""Plan publish-readiness audio polish from audio/transcript sidecars."""

from __future__ import annotations

import argparse
import json
import math
import re
from pathlib import Path
from typing import Any


def load_body(path: str) -> dict[str, Any]:
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def linear_gain(db: float) -> float:
    return math.pow(10.0, db / 20.0)


def transcript_segments(body: dict[str, Any]) -> list[dict[str, Any]]:
    return body.get("segments", [])


def speaker_stats(transcript: dict[str, Any]) -> list[dict[str, Any]]:
    stats: dict[str, dict[str, float]] = {}
    for seg in transcript_segments(transcript):
        speaker = str(seg.get("speaker_id") or seg.get("speaker") or "unknown")
        start = float(seg.get("start_s", seg.get("start", 0.0)))
        end = float(seg.get("end_s", seg.get("end", start)))
        words = len(re.findall(r"[A-Za-z0-9']+", seg.get("text", "")))
        item = stats.setdefault(speaker, {"duration_s": 0.0, "words": 0.0})
        item["duration_s"] += max(0.0, end - start)
        item["words"] += words
    return [
        {"speaker": speaker, "duration_s": round(v["duration_s"], 3), "words": int(v["words"])}
        for speaker, v in sorted(stats.items())
    ]


def weak_windows(audio: dict[str, Any], threshold_db: float) -> list[dict[str, Any]]:
    out = []
    for win in audio.get("windows", []):
        rms = float(win.get("rms_db", 0.0))
        if rms <= threshold_db:
            out.append({"start_s": float(win.get("start_s", 0.0)), "rms_db": rms})
    return out[:50]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--audio-energy", required=True)
    parser.add_argument("--transcript")
    parser.add_argument("--target-lufs", type=float, default=-16.0)
    parser.add_argument("--weak-db", type=float, default=-42.0)
    args = parser.parse_args()

    audio = load_body(args.audio_energy)
    transcript = load_body(args.transcript) if args.transcript else {}
    integrated = audio.get("loudness_integrated_lufs")
    loudness_gap = None if integrated is None else args.target_lufs - float(integrated)
    volume_op = None
    if loudness_gap is not None and abs(loudness_gap) >= 1.0:
        volume_op = {
            "kind": "Set Volume",
            "gain": round(linear_gain(loudness_gap), 3),
            "gain_db": round(loudness_gap, 2),
            "reason": "loudness_target_gap",
        }

    long_silences = [
        {
            "start_s": float(s.get("start_s", 0.0)),
            "end_s": float(s.get("end_s", 0.0)),
            "duration_s": round(float(s.get("end_s", 0.0)) - float(s.get("start_s", 0.0)), 3),
        }
        for s in audio.get("silences", [])
        if float(s.get("end_s", 0.0)) - float(s.get("start_s", 0.0)) >= 1.0
    ]
    speakers = speaker_stats(transcript) if transcript else []
    speaker_balance = {
        "status": "needs_review" if len(speakers) > 1 else "insufficient_data",
        "speakers": speakers,
        "note": "Audio-energy is mono/global in v1; speaker volume balance needs per-speaker review unless isolated tracks exist.",
    }

    recommendations = []
    if volume_op:
        recommendations.append(volume_op)
    if long_silences:
        recommendations.append({"kind": "Trim Silence", "count": len(long_silences), "reason": "long_silences"})
    recommendations.append({"kind": "Set Loudness Target", "target_lufs": args.target_lufs})

    print(
        json.dumps(
            {
                "status": "planned",
                "target_lufs": args.target_lufs,
                "loudness_integrated_lufs": integrated,
                "loudness_gap_db": None if loudness_gap is None else round(loudness_gap, 2),
                "volume_op": volume_op,
                "loudness_op": {"kind": "Set Loudness Target", "target_lufs": args.target_lufs},
                "long_silences": long_silences,
                "weak_windows": weak_windows(audio, args.weak_db),
                "speaker_balance": speaker_balance,
                "music_voice_balance_risks": [],
                "recommendations": recommendations,
                "graph_ops": [r["kind"] for r in recommendations if r["kind"] in {"Set Volume", "Set Loudness Target"}],
                "detected_only": ["denoise", "eq", "de_ess"],
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
