#!/usr/bin/env python3
"""Classify meeting transcript segments into highlight categories."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


PATTERNS = {
    "decision": r"\b(decided|agreed|approved|consensus|final answer|go with|ship it)\b",
    "action_item": r"\b(i will|we will|action item|next step|deadline|by friday|follow up|owner|responsible)\b",
    "risk": r"\b(risk|blocker|concern|worried|issue|problem|delayed|blocked)\b",
    "cut": r"\b(can you hear me|you'?re on mute|let me share|one second|good morning)\b",
}


def load_body(path: str) -> dict:
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--transcript", required=True)
    p.add_argument("--audience", choices=["executive", "team", "async"], default="team")
    args = p.parse_args()

    transcript = load_body(args.transcript)
    highlights = []
    for seg in transcript.get("segments", []):
        text = str(seg.get("text", ""))
        labels = [k for k, pat in PATTERNS.items() if re.search(pat, text, re.I)]
        if not labels:
            continue
        start = float(seg.get("start_s", seg.get("start", 0.0)))
        end = float(seg.get("end_s", seg.get("end", start)))
        keep = any(l in labels for l in ["decision", "action_item", "risk"])
        highlights.append({
            "start_s": round(start, 3),
            "end_s": round(end, 3),
            "labels": labels,
            "keep": keep,
            "text": text,
        })
    print(json.dumps({"audience": args.audience, "segments": highlights}, indent=2))


if __name__ == "__main__":
    main()
