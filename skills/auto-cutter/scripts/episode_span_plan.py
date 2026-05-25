#!/usr/bin/env python3
"""Plan candidate episode spans from a long transcript."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any


INTRO_RE = re.compile(r"\b(welcome|today we have|in this episode|this is episode|welcome back)\b", re.I)
OUTRO_RE = re.compile(r"\b(thanks for listening|that'?s all|see you next time|subscribe|follow us)\b", re.I)
RESET_RE = re.compile(r"\b(next episode|new episode|welcome back|now we'?re going to|different topic)\b", re.I)
REHEARSAL_RE = re.compile(
    r"\b("
    r"practice|practicing|cut|one more time|try again|start over|"
    r"hold it for me|don't mind me|am i supposed|is it on me|321|3 2 1|"
    r"what'?s the other one|look at that camera"
    r")\b",
    re.I,
)

MIN_PUBLISHABLE_DURATION_S = 12 * 60.0
MIN_SHORT_EPISODE_WITH_OUTRO_S = 4 * 60.0
REHEARSAL_SCAN_WINDOW_S = 180.0


def load_body(path: str | None) -> dict[str, Any]:
    if not path:
        return {}
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def segments(body: dict[str, Any]) -> list[dict[str, Any]]:
    out = []
    for idx, seg in enumerate(body.get("segments", [])):
        start = float(seg.get("start_s", seg.get("start", 0.0)))
        end = float(seg.get("end_s", seg.get("end", start)))
        text = str(seg.get("text", "")).strip()
        if text and end > start:
            out.append({"idx": idx, "start_s": start, "end_s": end, "text": text})
    return out


def long_breaks(audio: dict[str, Any]) -> list[dict[str, float]]:
    return [
        {"start_s": float(s.get("start_s", 0.0)), "end_s": float(s.get("end_s", 0.0))}
        for s in audio.get("silences", [])
        if float(s.get("end_s", 0.0)) - float(s.get("start_s", 0.0)) >= 8.0
    ]


def topics(body: dict[str, Any]) -> list[dict[str, Any]]:
    return body.get("topics", [])


def detect_boundaries(segs: list[dict[str, Any]], audio: dict[str, Any], topic_body: dict[str, Any]) -> list[dict[str, Any]]:
    boundaries = []
    for seg in segs:
        reasons = []
        if INTRO_RE.search(seg["text"]):
            reasons.append("intro_language")
        if RESET_RE.search(seg["text"]):
            reasons.append("topic_or_episode_reset_language")
        if reasons:
            boundaries.append({"time_s": seg["start_s"], "kind": "start", "confidence": 0.82, "evidence": seg["text"], "reasons": reasons})
        if OUTRO_RE.search(seg["text"]):
            boundaries.append({"time_s": seg["end_s"], "kind": "end", "confidence": 0.78, "evidence": seg["text"], "reasons": ["outro_language"]})
    for silence in long_breaks(audio):
        boundaries.append({"time_s": silence["end_s"], "kind": "start", "confidence": 0.65, "evidence": f"long break {silence['start_s']:.1f}-{silence['end_s']:.1f}", "reasons": ["long_break"]})
        boundaries.append({"time_s": silence["start_s"], "kind": "end", "confidence": 0.6, "evidence": f"long break {silence['start_s']:.1f}-{silence['end_s']:.1f}", "reasons": ["long_break"]})
    for topic in topics(topic_body):
        label = str(topic.get("label", ""))
        if RESET_RE.search(label):
            boundaries.append({"time_s": float(topic.get("start_s", 0.0)), "kind": "start", "confidence": 0.58, "evidence": label, "reasons": ["topic_reset"]})
    return sorted(boundaries, key=lambda b: (b["time_s"], b["kind"]))


def text_between(
    segs: list[dict[str, Any]], start_s: float, end_s: float, max_window_s: float | None = None
) -> str:
    scan_end = min(end_s, start_s + max_window_s) if max_window_s else end_s
    return " ".join(
        seg["text"]
        for seg in segs
        if seg["end_s"] >= start_s and seg["start_s"] <= scan_end
    )


def classify_span(span: dict[str, Any], segs: list[dict[str, Any]]) -> dict[str, Any]:
    # Boundary candidates are noisy; only sustained, non-rehearsal spans should become user choices.
    duration = float(span["duration_s"])
    reasons = list(span.get("reasons", []))
    rejection_reasons = []
    early_text = text_between(
        segs,
        float(span["start_s"]),
        float(span["end_s"]),
        max_window_s=REHEARSAL_SCAN_WINDOW_S,
    )

    if REHEARSAL_RE.search(early_text):
        rejection_reasons.append("rehearsal_or_false_start_language")

    has_outro = "outro_language" in reasons
    long_enough = duration >= MIN_PUBLISHABLE_DURATION_S
    short_episode_with_outro = has_outro and duration >= MIN_SHORT_EPISODE_WITH_OUTRO_S
    if not long_enough and not short_episode_with_outro:
        rejection_reasons.append("too_short_for_publishable_episode")

    if rejection_reasons:
        return {
            **span,
            "classification": "rejected",
            "publishable": False,
            "rejection_reasons": rejection_reasons,
        }
    return {**span, "classification": "publishable", "publishable": True}


def build_spans(segs: list[dict[str, Any]], boundaries: list[dict[str, Any]]) -> list[dict[str, Any]]:
    if not segs:
        return []
    starts = [b for b in boundaries if b["kind"] == "start"]
    if not starts:
        starts = [{"time_s": segs[0]["start_s"], "confidence": 0.45, "evidence": "first transcript segment", "reasons": ["fallback_start"]}]
    spans = []
    end_of_recording = segs[-1]["end_s"]
    for idx, start in enumerate(starts):
        next_start = starts[idx + 1]["time_s"] if idx + 1 < len(starts) else None
        candidate_ends = [b for b in boundaries if b["kind"] == "end" and b["time_s"] > start["time_s"] and (next_start is None or b["time_s"] <= next_start)]
        end_s = candidate_ends[-1]["time_s"] if candidate_ends else (next_start if next_start is not None else end_of_recording)
        if end_s - start["time_s"] < 20.0:
            continue
        confidence = min(0.95, float(start["confidence"]) + (0.1 if candidate_ends else 0.0))
        spans.append(
            {
                "label": f"episode_{len(spans) + 1}",
                "start_s": round(float(start["time_s"]), 3),
                "end_s": round(float(end_s), 3),
                "duration_s": round(float(end_s - start["time_s"]), 3),
                "confidence": round(confidence, 3),
                "evidence": [start["evidence"]] + [e["evidence"] for e in candidate_ends[-1:]],
                "reasons": start["reasons"] + ([candidate_ends[-1]["reasons"][0]] if candidate_ends else []),
            }
        )
    return spans


def publishable_plan(spans: list[dict[str, Any]], segs: list[dict[str, Any]]) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    classified = [classify_span(span, segs) for span in spans]
    episode_spans = []
    rejected_spans = []
    for span in classified:
        if span["publishable"]:
            episode_spans.append({**span, "label": f"episode_{len(episode_spans) + 1}"})
        else:
            rejected_spans.append(span)
    return episode_spans, rejected_spans


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--transcript", required=True)
    parser.add_argument("--audio-energy")
    parser.add_argument("--topic")
    args = parser.parse_args()

    transcript = load_body(args.transcript)
    audio = load_body(args.audio_energy)
    topic_body = load_body(args.topic)
    segs = segments(transcript)
    boundaries = detect_boundaries(segs, audio, topic_body)
    raw_spans = build_spans(segs, boundaries)
    spans, rejected_spans = publishable_plan(raw_spans, segs)
    high_conf = [s for s in spans if s["confidence"] >= 0.7]
    recommended = high_conf[0] if high_conf else (spans[0] if spans else None)
    print(
        json.dumps(
            {
                "status": "planned",
                "episode_spans": spans,
                "rejected_spans": rejected_spans,
                "recommended_span": recommended,
                "requires_user_choice": len(high_conf) > 1,
                "evidence": boundaries,
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
