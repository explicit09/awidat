#!/usr/bin/env python3
"""Generate pacing cuts and speed recommendations."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


CONFIG = {
    "short_form": {"silence_s": 0.5, "min_keep_s": 0.2},
    "talking_head": {"silence_s": 0.8, "min_keep_s": 0.25},
    "interview": {"silence_s": 1.0, "min_keep_s": 0.35},
    "podcast": {"silence_s": 1.2, "min_keep_s": 0.4},
    "tutorial": {"silence_s": 3.0, "min_keep_s": 0.6},
}


def load_body(path: str) -> dict:
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def wpm(text: str, start: float, end: float) -> float:
    words = re.findall(r"[A-Za-z0-9']+", text)
    return len(words) / max((end - start) / 60.0, 0.01)


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
    p.add_argument("--topic")
    p.add_argument("--shot")
    p.add_argument("--content-type", choices=CONFIG, default="interview")
    args = p.parse_args()

    cfg = CONFIG[args.content_type]
    energy = load_body(args.audio_energy)
    transcript = load_body(args.transcript)
    topic = load_body(args.topic) if args.topic else {}
    shot = load_body(args.shot) if args.shot else {}
    cuts = []
    speed_changes = []

    for s in energy.get("silences", []):
        start = float(s.get("start_s", 0.0))
        end = float(s.get("end_s", start))
        if end - start >= cfg["silence_s"]:
            cuts.append({
                "start_s": round(start + cfg["min_keep_s"] / 2, 3),
                "end_s": round(end - cfg["min_keep_s"] / 2, 3),
                "reason": "dead_zone",
                "topic_boundary": near_topic_boundary(topic, start, end),
            })

    for seg in transcript.get("segments", []):
        start = float(seg.get("start_s", seg.get("start", 0.0)))
        end = float(seg.get("end_s", seg.get("end", start)))
        rate = wpm(str(seg.get("text", "")), start, end)
        factor = speed_for(rate)
        if factor > 1.0 and not visually_dense(shot, start, end):
            speed_changes.append({
                "start_s": round(start, 3),
                "end_s": round(end, 3),
                "factor": factor,
                "wpm": round(rate, 1),
            })

    print(json.dumps({
        "content_type": args.content_type,
        "cuts": cuts,
        "speed_changes": speed_changes,
        "summary": {
            "cuts": len(cuts),
            "speed_changes": len(speed_changes),
        },
    }, indent=2))


def near_topic_boundary(topic: dict, start: float, end: float) -> bool:
    for t in topic.get("topics", []):
        ts = float(t.get("start_s", 0.0))
        te = float(t.get("end_s", ts))
        if abs(start - ts) < 2.0 or abs(end - te) < 2.0:
            return True
    return False


def visually_dense(shot: dict, start: float, end: float) -> bool:
    for s in shot.get("shots", []):
        if float(s.get("end_s", 0)) < start or float(s.get("start_s", 0)) > end:
            continue
        if s.get("motion") in {"fast-cut", "handheld"}:
            return True
    return False


if __name__ == "__main__":
    main()
