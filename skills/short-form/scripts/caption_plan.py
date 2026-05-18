#!/usr/bin/env python3
"""Group word-level transcript timing into short caption phrases."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from transcript_phrases import group_words_into_phrases, normalize_words


def load_body(path: str) -> dict:
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def words(body: dict) -> list[dict]:
    return normalize_words(body)


def build_caption_phrases(
    items: list[dict],
    *,
    max_words: int = 4,
    max_gap_s: float = 0.5,
    hot_start_s: float | None = None,
    hot_end_s: float | None = None,
) -> list[dict]:
    phrases = []
    for phrase in group_words_into_phrases(items, max_words=max_words, max_gap_s=max_gap_s):
        start = float(phrase.get("start_s", 0.0))
        end = float(phrase.get("end_s", start))
        hot = (
            hot_start_s is not None
            and hot_end_s is not None
            and end >= hot_start_s
            and start <= hot_end_s
        )
        phrases.append({
            "text": str(phrase.get("text", "")).strip(),
            "start_s": round(start, 3),
            "end_s": round(max(end, start + 0.6), 3),
            "position": "bottom",
            "font_size": 56,
            "color": "#FFD400" if hot else "#FFFFFF",
            "font_weight": "bold" if hot else "normal",
            "animation": "fade_in_out",
            "safe_area": "mobile",
        })
    return phrases


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--transcript", required=True)
    p.add_argument("--max-words", type=int, default=4)
    p.add_argument("--max-gap-s", type=float, default=0.5)
    p.add_argument("--hot-start-s", type=float)
    p.add_argument("--hot-end-s", type=float)
    args = p.parse_args()

    items = words(load_body(args.transcript))
    phrases = build_caption_phrases(
        items,
        max_words=args.max_words,
        max_gap_s=args.max_gap_s,
        hot_start_s=args.hot_start_s,
        hot_end_s=args.hot_end_s,
    )
    print(json.dumps({"phrases": phrases}, indent=2))


if __name__ == "__main__":
    main()
