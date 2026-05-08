#!/usr/bin/env python3
"""Build a conservative cleanup plan from audio-energy + transcript sidecars.

Outputs JSON:
  {"preset": "...", "cuts": [...], "summary": {...}}
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


PRESETS = {
    "gentle": {"silence_s": 1.2, "keep_s": 0.5},
    "standard": {"silence_s": 0.8, "keep_s": 0.35},
    "aggressive": {"silence_s": 0.45, "keep_s": 0.2},
}
FILLERS = {"um", "uh", "er", "ah", "hmm"}


def load_body(path: str | None) -> dict:
    if not path:
        return {}
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def transcript_words(body: dict) -> list[dict]:
    if body.get("words"):
        return body["words"]
    out = []
    for seg in body.get("segments", []):
        text = seg.get("text", "")
        start = float(seg.get("start_s", seg.get("start", 0.0)))
        end = float(seg.get("end_s", seg.get("end", start)))
        toks = re.findall(r"[A-Za-z']+", text)
        span = max(0.01, end - start)
        for i, tok in enumerate(toks):
            t0 = start + span * i / max(1, len(toks))
            t1 = start + span * (i + 1) / max(1, len(toks))
            out.append({"word": tok, "start_s": t0, "end_s": t1})
    return out


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--audio-energy")
    p.add_argument("--transcript")
    p.add_argument("--preset", choices=PRESETS, default="standard")
    args = p.parse_args()

    cfg = PRESETS[args.preset]
    energy = load_body(args.audio_energy)
    transcript = load_body(args.transcript)
    cuts = []

    for silence in energy.get("silences", []):
        start = float(silence.get("start_s", 0.0))
        end = float(silence.get("end_s", start))
        duration = end - start
        if duration >= cfg["silence_s"]:
            cuts.append({
                "start_s": round(start + cfg["keep_s"] / 2, 3),
                "end_s": round(end - cfg["keep_s"] / 2, 3),
                "duration_s": round(max(0.0, duration - cfg["keep_s"]), 3),
                "reason": "dead_air",
                "confidence": 0.9,
            })

    for word in transcript_words(transcript):
        token = re.sub(r"[^a-z']", "", str(word.get("word", "")).lower())
        if token in FILLERS:
            start = float(word.get("start_s", word.get("start", 0.0)))
            end = float(word.get("end_s", word.get("end", start)))
            cuts.append({
                "start_s": round(start, 3),
                "end_s": round(end, 3),
                "duration_s": round(max(0.0, end - start), 3),
                "reason": "filler_word",
                "text": token,
                "confidence": 0.75,
            })

    cuts.sort(key=lambda x: (x["start_s"], x["end_s"]))
    print(json.dumps({
        "preset": args.preset,
        "cuts": cuts,
        "summary": {
            "count": len(cuts),
            "total_duration_s": round(sum(c["duration_s"] for c in cuts), 3),
        },
    }, indent=2))


if __name__ == "__main__":
    main()
