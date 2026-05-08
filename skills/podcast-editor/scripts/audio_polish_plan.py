#!/usr/bin/env python3
"""Plan podcast polish edits from energy and transcript sidecars."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


THRESHOLDS = {
    "interview": {"silence_s": 1.0, "weak_db": -38.0},
    "podcast": {"silence_s": 0.9, "weak_db": -40.0},
    "solo": {"silence_s": 0.75, "weak_db": -36.0},
}


def load_body(path: str) -> dict:
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def segments(body: dict) -> list[dict]:
    return body.get("segments", [])


def wpm(seg: dict) -> float:
    text = seg.get("text", "")
    words = re.findall(r"[A-Za-z0-9']+", text)
    start = float(seg.get("start_s", seg.get("start", 0.0)))
    end = float(seg.get("end_s", seg.get("end", start)))
    minutes = max((end - start) / 60.0, 0.01)
    return len(words) / minutes


def speed_for(rate: float) -> float:
    if rate < 120:
        return 1.15
    if rate < 130:
        return 1.08
    return 1.0


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--audio-energy", required=True)
    p.add_argument("--transcript", required=True)
    p.add_argument("--content-type", choices=THRESHOLDS, default="interview")
    args = p.parse_args()

    cfg = THRESHOLDS[args.content_type]
    energy = load_body(args.audio_energy)
    transcript = load_body(args.transcript)
    cuts = []
    speed_changes = []

    for silence in energy.get("silences", []):
        start = float(silence.get("start_s", 0.0))
        end = float(silence.get("end_s", start))
        if end - start >= cfg["silence_s"]:
            cuts.append({
                "start_s": round(start + 0.15, 3),
                "end_s": round(end - 0.15, 3),
                "reason": "dead_air",
            })

    for seg in segments(transcript):
        rate = wpm(seg)
        factor = speed_for(rate)
        if factor > 1.0:
            speed_changes.append({
                "start_s": round(float(seg.get("start_s", seg.get("start", 0.0))), 3),
                "end_s": round(float(seg.get("end_s", seg.get("end", 0.0))), 3),
                "factor": factor,
                "wpm": round(rate, 1),
                "reason": "slow_delivery",
            })

    print(json.dumps({
        "content_type": args.content_type,
        "cuts": cuts,
        "speed_changes": speed_changes,
        "summary": {
            "cut_count": len(cuts),
            "speed_change_count": len(speed_changes),
        },
    }, indent=2))


if __name__ == "__main__":
    main()
