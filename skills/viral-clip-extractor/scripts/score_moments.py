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


def score_candidates(
    *,
    moments_body: dict,
    energy_body: dict,
    transcript_body: dict | None = None,
    shot_body: dict | None = None,
    gaze_body: dict | None = None,
    quality_body: dict | None = None,
    topic_body: dict | None = None,
    limit: int = 8,
    max_overlap_ratio: float | None = None,
) -> list[dict]:
    if max_overlap_ratio is not None and not 0.0 <= max_overlap_ratio <= 1.0:
        raise ValueError("max_overlap_ratio must be between 0 and 1")

    transcript = transcript_body or {}
    shot = shot_body or {}
    gaze = gaze_body or {}
    quality = quality_body or {}
    topic = topic_body or {}
    scored = []
    for m in moments(moments_body):
        start = float(m.get("start_s", 0.0))
        end = float(m.get("end_s", start))
        text = text_for(transcript, start, end)
        kind = str(m.get("kind", "story"))
        base = float(m.get("score", 0.5)) * 40.0
        kind_score = KIND_WEIGHT.get(kind, 8)
        energy_normalized = avg_energy(energy_body, start, end)
        energy_score = energy_normalized * 0.35
        hook = 12 if HOOK_RE.search(text + " " + str(m.get("note", ""))) else 0
        duration = end - start
        duration_bonus = 8 if 20 <= duration <= 90 else -8
        visual = visual_breakdown(shot, gaze, quality, start, end)
        topic_bonus = 5 if overlaps_topic(topic, start, end) else 0
        total = (
            base
            + energy_score
            + visual["total"]
            + kind_score
            + hook
            + duration_bonus
            + topic_bonus
        )
        rounded_total = round(total, 2)
        scored.append({
            "moment_id": m.get("moment_id", m.get("id", "")),
            "kind": kind,
            "start_s": round(start, 3),
            "end_s": round(end, 3),
            "duration_s": round(duration, 3),
            "score": rounded_total,
            "score_breakdown": {
                "base": round(base, 2),
                "kind": round(kind_score, 2),
                "energy": round(energy_score, 2),
                "visual": round(visual["total"], 2),
                "hook": round(hook, 2),
                "duration": round(duration_bonus, 2),
                "topic": round(topic_bonus, 2),
                "topic_boundary": bool(topic_bonus),
                "total": rounded_total,
            },
            "visual_breakdown": {key: round(value, 2) for key, value in visual.items()},
            "energy_score": round(energy_normalized, 2),
            "visual_score": round(visual["total"], 2),
            "topic_boundary_bonus": bool(topic_bonus),
            "hook_signal": bool(hook),
            "dependencies": m.get("dependencies", []),
            "note": m.get("note"),
        })
    scored.sort(key=lambda x: x["score"], reverse=True)
    if max_overlap_ratio is not None:
        scored = suppress_overlapping_candidates(scored, max_overlap_ratio)
    return scored[:limit]


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
    p.add_argument(
        "--max-overlap-ratio",
        type=float,
        help="Drop lower-scored candidates that overlap a kept candidate above this ratio.",
    )
    args = p.parse_args()

    mbody = load_body(args.moments)
    energy = load_body(args.audio_energy)
    transcript = load_body(args.transcript) if args.transcript else {}
    shot = load_body(args.shot) if args.shot else {}
    gaze = load_body(args.gaze) if args.gaze else {}
    quality = load_body(args.frame_quality) if args.frame_quality else {}
    topic = load_body(args.topic) if args.topic else {}
    candidates = score_candidates(
        moments_body=mbody,
        energy_body=energy,
        transcript_body=transcript,
        shot_body=shot,
        gaze_body=gaze,
        quality_body=quality,
        topic_body=topic,
        limit=args.limit,
        max_overlap_ratio=args.max_overlap_ratio,
    )
    print(json.dumps({"candidates": candidates}, indent=2))


def visual_score(shot: dict, gaze: dict, quality: dict, start: float, end: float) -> float:
    return visual_breakdown(shot, gaze, quality, start, end)["total"]


def visual_breakdown(
    shot: dict,
    gaze: dict,
    quality: dict,
    start: float,
    end: float,
) -> dict[str, float]:
    breakdown = {
        "shot_type": 0.0,
        "action": 0.0,
        "cutaway": 0.0,
        "face": 0.0,
        "quality": 0.0,
        "total": 0.0,
    }
    shots = [
        s
        for s in shot.get("shots", [])
        if float(s.get("end_s", 0)) >= start and float(s.get("start_s", 0)) <= end
    ]
    if shots:
        if any(s.get("type") in {"close-up", "medium"} for s in shots):
            breakdown["shot_type"] = 5.0
        if any(s.get("motion") in {"slow-pan", "handheld", "fast-cut"} for s in shots):
            breakdown["action"] = 4.0
        if any(s.get("type") in {"no-face", "wide"} for s in shots):
            breakdown["cutaway"] = 3.0
    frames = [f for f in gaze.get("per_frame", []) if start <= float(f.get("t_s", 0)) <= end]
    at_camera = sum(1 for f in frames for face in f.get("faces", []) if face.get("at_camera"))
    if frames and at_camera / max(1, len(frames)) > 0.2:
        breakdown["face"] = 6.0
    qframes = [f for f in quality.get("per_frame", []) if start <= float(f.get("t_s", 0)) <= end]
    if qframes:
        sharp = sum(1 for f in qframes if f.get("is_sharp")) / len(qframes)
        if sharp > 0.75:
            breakdown["quality"] = 4.0
    score = sum(value for key, value in breakdown.items() if key != "total")
    breakdown["total"] = score
    return breakdown


def overlaps_topic(topic: dict, start: float, end: float) -> bool:
    for t in topic.get("topics", []):
        ts = float(t.get("start_s", 0.0))
        te = float(t.get("end_s", ts))
        if abs(start - ts) < 5 or abs(end - te) < 5:
            return True
    return False


def suppress_overlapping_candidates(
    candidates: list[dict],
    max_overlap_ratio: float,
) -> list[dict]:
    if not 0.0 <= max_overlap_ratio <= 1.0:
        raise ValueError("max_overlap_ratio must be between 0 and 1")

    kept: list[dict] = []
    for candidate in candidates:
        if not any(
            overlap_ratio(candidate, existing) > max_overlap_ratio
            for existing in kept
        ):
            kept.append(candidate)
    return kept


def overlap_ratio(left: dict, right: dict) -> float:
    left_start = float(left.get("start_s", 0.0))
    left_end = float(left.get("end_s", left_start))
    right_start = float(right.get("start_s", 0.0))
    right_end = float(right.get("end_s", right_start))
    overlap = max(0.0, min(left_end, right_end) - max(left_start, right_start))
    shorter = min(max(0.0, left_end - left_start), max(0.0, right_end - right_start))
    if shorter == 0.0:
        return 0.0
    return overlap / shorter


if __name__ == "__main__":
    main()
