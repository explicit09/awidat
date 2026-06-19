#!/usr/bin/env python3
"""Build a semantic episode-flow review packet from transcript evidence.

This script does not decide the final episode spans from regex matches.
It gathers boundary hints and transcript context, then returns a blocking
review contract that the active editor/LLM must resolve before timeline edits.
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any


BOUNDARY_HINTS = (
    ("possible_start", re.compile(r"\b(today we are|today we're|in this episode|welcome back|let'?s hear it)\b", re.I)),
    ("possible_close", re.compile(r"\b(thanks for tuning in|thanks for listening|today'?s episode|that'?s a wrap|until next time|see you next time)\b", re.I)),
    ("possible_reset", re.compile(r"\b(which ones are we doing|what was the topic|are you ready for|next topic|new episode|different topic|agent loop)\b", re.I)),
    ("production_meta", re.compile(r"\b(the clip|the edit|thumbnail|upload|retention|caption|b-?roll|our podcast|our episode)\b", re.I)),
)

REQUIRED_FIELDS = (
    "recording_shape",
    "episode_spans",
    "clip_candidates",
    "post_show_or_production_spans",
    "confidence",
    "decision",
)


def load_body(path: str) -> dict[str, Any]:
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def segments(body: dict[str, Any]) -> list[dict[str, Any]]:
    rows = []
    for idx, seg in enumerate(body.get("segments", [])):
        start = float(seg.get("start_s", seg.get("start", 0.0)))
        end = float(seg.get("end_s", seg.get("end", start)))
        text = str(seg.get("text", "")).strip()
        if text and end > start:
            rows.append({"idx": idx, "start_s": start, "end_s": end, "text": text})
    return rows


def boundary_hints(segs: list[dict[str, Any]]) -> list[dict[str, Any]]:
    hints = []
    for seg in segs:
        reasons = [name for name, pattern in BOUNDARY_HINTS if pattern.search(seg["text"])]
        if reasons:
            hints.append(
                {
                    "start_s": seg["start_s"],
                    "end_s": seg["end_s"],
                    "reasons": reasons,
                    "text": seg["text"],
                }
            )
    return hints


def excerpt_for_review(segs: list[dict[str, Any]], hints: list[dict[str, Any]]) -> str:
    if not hints:
        selected_indexes = set(range(min(len(segs), 8)))
        if len(segs) > 8:
            midpoint = len(segs) // 2
            selected_indexes.update(range(max(0, midpoint - 4), min(len(segs), midpoint + 4)))
            selected_indexes.update(range(max(0, len(segs) - 8), len(segs)))
        selected = [segs[idx] for idx in sorted(selected_indexes)]
    else:
        selected_indexes = set()
        for hint in hints:
            center = min(
                range(len(segs)),
                key=lambda idx: abs(segs[idx]["start_s"] - hint["start_s"]),
            )
            selected_indexes.update(range(max(0, center - 2), min(len(segs), center + 3)))
        selected = [segs[idx] for idx in sorted(selected_indexes)]

    lines = []
    for seg in selected:
        lines.append(f"[{seg['start_s']:.3f}-{seg['end_s']:.3f}] {seg['text']}")
    return "\n".join(lines)


def review_contract() -> dict[str, Any]:
    return {
        "required_fields": list(REQUIRED_FIELDS),
        "instructions": [
            "Classify the recording shape from transcript flow, not keyword presence.",
            "Return every publishable episode span separately with source start/end seconds.",
            "Separate full episodes from short clip candidates and post-show production talk.",
            "If two or more publishable episode spans exist, set decision to requires_user_choice before timeline edits.",
            "If evidence is weak, set decision to needs_more_review instead of guessing.",
        ],
        "episode_span_schema": {
            "label": "episode_1",
            "start_s": 0.0,
            "end_s": 0.0,
            "topic": "short topic label",
            "evidence": ["timestamped transcript reasons"],
            "confidence": "low|medium|high",
        },
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--transcript", required=True)
    parser.add_argument("--asset-id")
    args = parser.parse_args()

    segs = segments(load_body(args.transcript))
    hints = boundary_hints(segs)
    status = "needs_semantic_review" if segs else "missing_transcript"
    report = {
        "status": status,
        "summary_for_agent": (
            "Episode flow shape requires semantic transcript review before timeline edits. "
            f"Found {len(hints)} boundary hint(s); treat them as recall evidence only."
        ),
        "asset_id": args.asset_id,
        "blocks_timeline_edits": True,
        "required_passes": [
            "mechanical_boundary_recall",
            "semantic_flow_review",
            "adversarial_scope_check",
            "post_edit_shape_verification",
        ],
        "candidate_boundaries": hints,
        "llm_review_packet": {
            "transcript_excerpt": excerpt_for_review(segs, hints),
            "review_question": (
                "How many publishable episodes are in this recording, what are their source spans, "
                "and which parts are only clips or production planning?"
            ),
        },
        "llm_review_contract": review_contract(),
        "next_step": (
            "Complete the semantic flow review contract. Do not extract, clean up, or render until "
            "the contract identifies one chosen episode span or explicitly asks the user to choose."
        ),
    }
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
