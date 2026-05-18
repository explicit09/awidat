#!/usr/bin/env python3
"""Shared transcript phrase grouping helpers for Awidat skills."""

from __future__ import annotations

import re
from typing import Iterable


WORD_RE = re.compile(r"[A-Za-z0-9']+|[.,!?;:]")
TERMINAL_RE = re.compile(r"[.!?]$")


def _float_value(item: dict, *keys: str, default: float = 0.0) -> float:
    for key in keys:
        if key in item and item[key] is not None:
            return float(item[key])
    return default


def _word_text(item: dict) -> str:
    text = str(item.get("word", item.get("text", ""))).strip()
    return text


def _word_tokens_with_punctuation(text: str) -> list[str]:
    tokens: list[str] = []
    for token in WORD_RE.findall(text):
        if re.match(r"[A-Za-z0-9']", token):
            tokens.append(token)
        elif tokens and token in {".", "!", "?"}:
            tokens[-1] = f"{tokens[-1]}{token}"
    return tokens


def normalize_words(body_or_words: dict | Iterable[dict]) -> list[dict]:
    """Normalize common transcript shapes into timed word dictionaries."""
    if isinstance(body_or_words, dict):
        if body_or_words.get("words"):
            raw_words = body_or_words["words"]
        else:
            raw_words = []
            for seg in body_or_words.get("segments", []):
                text = str(seg.get("text", ""))
                tokens = _word_tokens_with_punctuation(text)
                start = _float_value(seg, "start_s", "start")
                end = _float_value(seg, "end_s", "end", default=start)
                span = max(0.01, end - start)
                for index, token in enumerate(tokens):
                    raw_words.append({
                        "word": token,
                        "start_s": start + span * index / max(1, len(tokens)),
                        "end_s": start + span * (index + 1) / max(1, len(tokens)),
                        "speaker": seg.get("speaker"),
                    })
    else:
        raw_words = list(body_or_words)

    words = []
    for item in raw_words:
        text = _word_text(item)
        if not text:
            continue
        start = _float_value(item, "start_s", "start")
        end = _float_value(item, "end_s", "end", default=start)
        words.append({
            "word": text,
            "start_s": start,
            "end_s": max(end, start),
            "speaker": item.get("speaker"),
        })
    return sorted(words, key=lambda word: (word["start_s"], word["end_s"]))


def group_words_into_phrases(
    body_or_words: dict | Iterable[dict],
    *,
    max_words: int = 4,
    max_gap_s: float = 0.5,
) -> list[dict]:
    """Group transcript words into short readable phrases."""
    words = normalize_words(body_or_words)
    phrases: list[dict] = []
    current: list[dict] = []

    def flush() -> None:
        if not current:
            return
        text = " ".join(str(word["word"]).strip() for word in current).strip()
        phrases.append({
            "text": text,
            "start_s": round(float(current[0]["start_s"]), 3),
            "end_s": round(float(current[-1]["end_s"]), 3),
            "speaker": current[0].get("speaker"),
            "word_count": len(current),
        })
        current.clear()

    for word in words:
        if current:
            previous = current[-1]
            gap_s = float(word["start_s"]) - float(previous["end_s"])
            speaker_changed = word.get("speaker") != previous.get("speaker")
            if speaker_changed or gap_s >= max_gap_s or len(current) >= max(1, max_words):
                flush()

        current.append(word)
        if TERMINAL_RE.search(str(word["word"])):
            flush()

    flush()
    return phrases


def render_packed_markdown(
    sources: Iterable[tuple[str, dict | Iterable[dict]]],
    *,
    max_words: int = 18,
    max_gap_s: float = 0.5,
) -> str:
    """Render grouped transcript phrases as compact markdown."""
    lines = ["# Packed transcripts", ""]
    for label, body in sources:
        lines.extend([f"## {label}", ""])
        for phrase in group_words_into_phrases(body, max_words=max_words, max_gap_s=max_gap_s):
            speaker = phrase.get("speaker")
            speaker_prefix = f"{speaker} " if speaker else ""
            lines.append(
                f"[{phrase['start_s']:.2f}-{phrase['end_s']:.2f}] "
                f"{speaker_prefix}{phrase['text']}"
            )
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"
