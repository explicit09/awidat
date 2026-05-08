#!/usr/bin/env python3
"""Score editorial moments for social clip extraction."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


KIND_WEIGHT = {
    "hook": 30,
    "punchline": 24,
    "emotional_peak": 22,
    "story": 18,
    "question": 16,
    "answer": 14,
    "cta": 10,
    "tangent": -10,
    "dead_air": -30,
}
HOOK_RE = re.compile(r"\?|actually|truth|never|always|mistake|realized|secret|problem|why", re.I)


def load_body(path: str) -> dict:
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def moments(body: dict) -> list[dict]:
    return body.get("moments", body.get("results", []))


def transcript_segments(body: dict) -> list[dict]:
    return body.get("segments", [])


def avg_energy(energy: dict, start: float, end: float) -> float:
    vals = [
        float(w.get("rms_db", -80))
        for w in energy.get("windows", [])
        if start <= float(w.get("start_s", 0.0)) <= end
    ]
    if not vals:
        return 0.0
    avg_db = sum(vals) / len(vals)
    return max(0.0, min(100.0, (avg_db + 60.0) * 2.0))


def text_for(transcript: dict, start: float, end: float) -> str:
    chunks = []
    for seg in transcript_segments(transcript):
        s = float(seg.get("start_s", seg.get("start", 0.0)))
        e = float(seg.get("end_s", seg.get("end", s)))
        if e >= start and s <= end:
            chunks.append(str(seg.get("text", "")))
    return " ".join(chunks)


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--moments", required=True)
    p.add_argument("--audio-energy", required=True)
    p.add_argument("--transcript")
    p.add_argument("--shot")
    p.add_argument("--gaze")
    p.add_argument("--frame-quality")
    p.add_argument("--topic")
    p.add_argument("--limit", type=int, default=8)
    args = p.parse_args()

    mbody = load_body(args.moments)
    energy = load_body(args.audio_energy)
    transcript = load_body(args.transcript) if args.transcript else {}
    shot = load_body(args.shot) if args.shot else {}
    gaze = load_body(args.gaze) if args.gaze else {}
    quality = load_body(args.frame_quality) if args.frame_quality else {}
    topic = load_body(args.topic) if args.topic else {}
    scored = []
    for m in moments(mbody):
        start = float(m.get("start_s", 0.0))
        end = float(m.get("end_s", start))
        text = text_for(transcript, start, end)
        kind = str(m.get("kind", "story"))
        base = float(m.get("score", 0.5)) * 40.0
        e_score = avg_energy(energy, start, end) * 0.35
        hook = 12 if HOOK_RE.search(text + " " + str(m.get("note", ""))) else 0
        duration = end - start
        duration_bonus = 8 if 20 <= duration <= 90 else -8
        visual = visual_score(shot, gaze, quality, start, end)
        topic_bonus = 5 if overlaps_topic(topic, start, end) else 0
        total = base + e_score + visual + KIND_WEIGHT.get(kind, 8) + hook + duration_bonus + topic_bonus
        scored.append({
            "moment_id": m.get("moment_id", m.get("id", "")),
            "kind": kind,
            "start_s": round(start, 3),
            "end_s": round(end, 3),
            "duration_s": round(duration, 3),
            "score": round(total, 2),
            "energy_score": round(e_score / 0.35 if e_score else 0.0, 2),
            "visual_score": round(visual, 2),
            "topic_boundary_bonus": bool(topic_bonus),
            "hook_signal": bool(hook),
            "dependencies": m.get("dependencies", []),
            "note": m.get("note"),
        })
    scored.sort(key=lambda x: x["score"], reverse=True)
    print(json.dumps({"candidates": scored[: args.limit]}, indent=2))


def visual_score(shot: dict, gaze: dict, quality: dict, start: float, end: float) -> float:
    score = 0.0
    shots = [s for s in shot.get("shots", []) if float(s.get("end_s", 0)) >= start and float(s.get("start_s", 0)) <= end]
    if shots:
        if any(s.get("type") in {"close-up", "medium"} for s in shots):
            score += 5
        if any(s.get("motion") in {"slow-pan", "handheld", "fast-cut"} for s in shots):
            score += 4
        if any(s.get("type") in {"no-face", "wide"} for s in shots):
            score += 3
    frames = [f for f in gaze.get("per_frame", []) if start <= float(f.get("t_s", 0)) <= end]
    at_camera = sum(1 for f in frames for face in f.get("faces", []) if face.get("at_camera"))
    if frames and at_camera / max(1, len(frames)) > 0.2:
        score += 6
    qframes = [f for f in quality.get("per_frame", []) if start <= float(f.get("t_s", 0)) <= end]
    if qframes:
        sharp = sum(1 for f in qframes if f.get("is_sharp")) / len(qframes)
        if sharp > 0.75:
            score += 4
    return score


def overlaps_topic(topic: dict, start: float, end: float) -> bool:
    for t in topic.get("topics", []):
        ts = float(t.get("start_s", 0.0))
        te = float(t.get("end_s", ts))
        if abs(start - ts) < 5 or abs(end - te) < 5:
            return True
    return False


if __name__ == "__main__":
    main()
