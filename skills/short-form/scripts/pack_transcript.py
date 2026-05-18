#!/usr/bin/env python3
"""Render transcript sidecars into compact phrase-level markdown."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from transcript_phrases import render_packed_markdown


def load_body(path: str) -> dict:
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def build_packed_transcript(
    sources: list[tuple[str, dict]],
    *,
    max_words: int = 18,
    max_gap_s: float = 0.5,
) -> str:
    return render_packed_markdown(sources, max_words=max_words, max_gap_s=max_gap_s)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--transcript", action="append", required=True)
    parser.add_argument("--source-label", action="append", default=[])
    parser.add_argument("--max-words", type=int, default=18)
    parser.add_argument("--max-gap-s", type=float, default=0.5)
    args = parser.parse_args()

    sources = []
    for index, transcript_path in enumerate(args.transcript):
        if index < len(args.source_label):
            label = args.source_label[index]
        else:
            label = Path(transcript_path).stem
        sources.append((label, load_body(transcript_path)))

    print(
        build_packed_transcript(
            sources,
            max_words=args.max_words,
            max_gap_s=args.max_gap_s,
        ),
        end="",
    )


if __name__ == "__main__":
    main()
