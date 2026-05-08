#!/usr/bin/env python3
"""Score rough-cut takes from transcript and silence sidecars."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


def load_body(path: str) -> dict:
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def avg_energy(energy: dict, start: float, end: float) -> float:
    vals = [float(w.get("rms_db", -80)) for w in energy.get("windows", []) if start <= float(w.get("start_s", 0)) <= end]
    if not vals:
        return 0.0
    return max(0.0, min(100.0, (sum(vals) / len(vals) + 60) * 2))


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--audio-energy", required=True)
    p.add_argument("--transcript", required=True)
    p.add_argument("--frame-quality")
    p.add_argument("--gaze")
    args = p.parse_args()

    energy = load_body(args.audio_energy)
    transcript = load_body(args.transcript)
    quality = load_body(args.frame_quality) if args.frame_quality else {}
    gaze = load_body(args.gaze) if args.gaze else {}
    takes = []
    for i, seg in enumerate(transcript.get("segments", []), 1):
        start = float(seg.get("start_s", seg.get("start", 0.0)))
        end = float(seg.get("end_s", seg.get("end", start)))
        text = str(seg.get("text", "")).strip()
        words = re.findall(r"[A-Za-z0-9']+", text)
        complete = bool(re.search(r"[.!?]$", text))
        vscore = visual_score(quality, gaze, start, end)
        score = avg_energy(energy, start, end) * 0.4 + min(len(words), 80) * 0.5 + vscore + (15 if complete else -10)
        takes.append({
            "take_id": f"take_{i:03d}",
            "start_s": round(start, 3),
            "end_s": round(end, 3),
            "duration_s": round(end - start, 3),
            "word_count": len(words),
            "complete_thought": complete,
            "visual_score": round(vscore, 2),
            "score": round(score, 2),
            "label": text[:64],
        })
    takes.sort(key=lambda t: t["score"], reverse=True)
    print(json.dumps({"takes": takes}, indent=2))


def visual_score(quality: dict, gaze: dict, start: float, end: float) -> float:
    score = 0.0
    qframes = [f for f in quality.get("per_frame", []) if start <= float(f.get("t_s", 0)) <= end]
    if qframes:
        sharp = sum(1 for f in qframes if f.get("is_sharp")) / len(qframes)
        if sharp > 0.75:
            score += 8
    gframes = [f for f in gaze.get("per_frame", []) if start <= float(f.get("t_s", 0)) <= end]
    if gframes:
        at_camera = sum(1 for f in gframes for face in f.get("faces", []) if face.get("at_camera"))
        if at_camera / max(1, len(gframes)) > 0.2:
            score += 8
    return score


if __name__ == "__main__":
    main()
