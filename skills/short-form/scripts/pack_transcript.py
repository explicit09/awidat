#!/usr/bin/env python3
"""Render transcript sidecars into compact phrase-level markdown."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from transcript_phrases import (
    group_words_into_phrases,
    normalize_words,
    render_packed_markdown,
)


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


def _freshness(body: dict) -> str:
    asset_hash = body.get("asset_sha256") or body.get("source_asset_sha256")
    indexed_hash = body.get("indexed_asset_sha256") or body.get("asset_sha256")
    if not asset_hash or not indexed_hash:
        return "unknown"
    return "fresh" if asset_hash == indexed_hash else "stale"


def _truncate(text: str, max_chars: int) -> tuple[str, bool]:
    if max_chars <= 0 or len(text) <= max_chars:
        return text, False
    suffix = "..."
    if max_chars <= len(suffix):
        return text[:max_chars], True
    return text[: max_chars - len(suffix)].rstrip() + suffix, True


def build_transcript_packet(
    sources: list[tuple[str, dict]],
    *,
    max_words: int = 18,
    max_gap_s: float = 0.5,
    max_chars: int = 12000,
) -> dict:
    markdown = build_packed_transcript(
        sources,
        max_words=max_words,
        max_gap_s=max_gap_s,
    )
    bounded_markdown, truncated = _truncate(markdown, max_chars)
    source_reports = []
    for label, body in sources:
        source_reports.append({
            "label": label,
            "freshness": _freshness(body),
            "word_count": len(normalize_words(body)),
            "phrase_count": len(group_words_into_phrases(
                body,
                max_words=max_words,
                max_gap_s=max_gap_s,
            )),
        })
    return {
        "status": "truncated" if truncated else "ready",
        "source_count": len(source_reports),
        "max_chars": max_chars,
        "markdown_chars": len(bounded_markdown),
        "sources": source_reports,
        "markdown": bounded_markdown,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--transcript", action="append", required=True)
    parser.add_argument("--source-label", action="append", default=[])
    parser.add_argument("--max-words", type=int, default=18)
    parser.add_argument("--max-gap-s", type=float, default=0.5)
    parser.add_argument("--max-chars", type=int, default=12000)
    parser.add_argument("--output-format", choices=("markdown", "json"), default="markdown")
    args = parser.parse_args()

    sources = []
    for index, transcript_path in enumerate(args.transcript):
        if index < len(args.source_label):
            label = args.source_label[index]
        else:
            label = Path(transcript_path).stem
        sources.append((label, load_body(transcript_path)))

    if args.output_format == "json":
        print(json.dumps(
            build_transcript_packet(
                sources,
                max_words=args.max_words,
                max_gap_s=args.max_gap_s,
                max_chars=args.max_chars,
            ),
            indent=2,
        ))
    else:
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
