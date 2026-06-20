#!/usr/bin/env python3
"""Shared transcript phrase grouping helpers for Montage skills."""

from __future__ import annotations

import re
from typing import Iterable


WORD_RE = re.compile(r"[A-Za-z0-9']+|[.,!?;:]")
TERMINAL_RE = re.compile(r"[.!?]$")
SOFT_PUNCTUATION_RE = re.compile(r"[,;:]$")


PHRASE_PRESETS = {
    "short": {
        "min_words": 3,
        "max_words": 4,
        "target_duration_s": 1.4,
        "max_duration_s": 2.6,
        "pause_break_s": 0.26,
    },
    "medium": {
        "min_words": 5,
        "max_words": 7,
        "target_duration_s": 2.3,
        "max_duration_s": 3.9,
        "pause_break_s": 0.38,
    },
    "long": {
        "min_words": 8,
        "max_words": 12,
        "target_duration_s": 3.3,
        "max_duration_s": 5.6,
        "pause_break_s": 0.52,
    },
}


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
                        "timing_source": "segment_interpolation",
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
        word = {
            "word": text,
            "start_s": start,
            "end_s": max(end, start),
            "speaker": item.get("speaker"),
        }
        if item.get("timing_source"):
            word["timing_source"] = item["timing_source"]
        words.append(word)
    return sorted(words, key=lambda word: (word["start_s"], word["end_s"]))


def group_words_into_phrases(
    body_or_words: dict | Iterable[dict],
    *,
    max_words: int = 4,
    max_gap_s: float = 0.5,
    phrase_preset: str | None = None,
) -> list[dict]:
    """Group transcript words into short readable phrases."""
    words = normalize_words(body_or_words)
    phrases: list[dict] = []
    current: list[dict] = []
    preset = PHRASE_PRESETS.get(phrase_preset or "")

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
            "word_timings": [
                {
                    "text": str(word["word"]).strip(),
                    "start_s": round(float(word["start_s"]), 3),
                    "end_s": round(float(word["end_s"]), 3),
                    **(
                        {"timing_source": word["timing_source"]}
                        if word.get("timing_source")
                        else {}
                    ),
                }
                for word in current
            ],
        })
        current.clear()

    def should_break_for_preset() -> bool:
        if not current or not preset:
            return False
        count = len(current)
        start = float(current[0]["start_s"])
        end = float(current[-1]["end_s"])
        duration_s = max(0.0, end - start)
        token = str(current[-1]["word"])

        if TERMINAL_RE.search(token):
            return True
        if count >= int(preset["max_words"]):
            return True
        if duration_s >= float(preset["max_duration_s"]):
            return True
        min_words = int(preset["min_words"])
        if count >= min_words and duration_s >= float(preset["target_duration_s"]):
            return True
        return (
            SOFT_PUNCTUATION_RE.search(token) is not None
            and count >= 2
            and duration_s >= float(preset["target_duration_s"]) * 0.7
        )

    for word in words:
        if current:
            previous = current[-1]
            gap_s = float(word["start_s"]) - float(previous["end_s"])
            speaker_changed = word.get("speaker") != previous.get("speaker")
            gap_limit_s = float(preset["pause_break_s"]) if preset else max_gap_s
            legacy_word_limit = not preset and len(current) >= max(1, max_words)
            if speaker_changed or gap_s >= gap_limit_s or legacy_word_limit:
                flush()

        current.append(word)
        if preset:
            if should_break_for_preset():
                flush()
        elif TERMINAL_RE.search(str(word["word"])):
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
