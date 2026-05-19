#!/usr/bin/env python3
"""Generate word-level caption progress evidence."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from transcript_phrases import normalize_words


SUPPORTED_STYLES = {"karaoke_fill", "word_highlight", "word_by_word"}


def _issue(code: str, message: str, *, index: int | None = None) -> dict:
    payload = {"code": code, "message": message}
    if index is not None:
        payload["index"] = index
    return payload


def _validate_words(words: list[dict]) -> list[dict]:
    issues = []
    previous_start = -1.0
    for index, word in enumerate(words):
        start = float(word.get("start_s", 0.0))
        end = float(word.get("end_s", start))
        if start < previous_start:
            issues.append(_issue(
                "unsorted_words",
                "word timings must be sorted by start_s",
                index=index,
            ))
        if end <= start:
            issues.append(_issue(
                "invalid_word_timing",
                "word end_s must be greater than start_s",
                index=index,
            ))
        previous_start = start
    return issues


def _word_state(word: dict, time_s: float) -> tuple[str, float]:
    start = float(word["start_s"])
    end = float(word["end_s"])
    if time_s < start:
        return "pending", 0.0
    if time_s >= end:
        return "completed", 1.0
    progress = (time_s - start) / (end - start)
    return "active", round(progress, 3)


def build_progress_report(
    transcript: dict,
    *,
    sample_times: list[float],
    style: str = "karaoke_fill",
) -> dict:
    if style not in SUPPORTED_STYLES:
        return {
            "version": 1,
            "status": "blocked",
            "style": style,
            "issues": [_issue("unsupported_style", f"unsupported caption progress style: {style}")],
            "frames": [],
        }
    raw_words = transcript.get("words") if isinstance(transcript, dict) else None
    raw_issues = _validate_words(raw_words) if isinstance(raw_words, list) else []
    words = normalize_words(transcript)
    issues = raw_issues + _validate_words(words)
    if issues:
        return {
            "version": 1,
            "status": "blocked",
            "style": style,
            "word_count": len(words),
            "issue_count": len(issues),
            "issues": issues,
            "frames": [],
        }
    frames = []
    for time_s in sorted(float(value) for value in sample_times):
        word_reports = []
        active_word = None
        for index, word in enumerate(words):
            state, progress = _word_state(word, time_s)
            if state == "active":
                active_word = str(word["word"])
            word_reports.append({
                "index": index,
                "text": str(word["word"]),
                "start_s": round(float(word["start_s"]), 3),
                "end_s": round(float(word["end_s"]), 3),
                "state": state,
                "progress": progress,
            })
        frames.append({
            "time_s": round(time_s, 3),
            "active_word": active_word,
            "words": word_reports,
        })
    return {
        "version": 1,
        "status": "ready",
        "style": style,
        "word_count": len(words),
        "frame_count": len(frames),
        "issue_count": 0,
        "issues": [],
        "frames": frames,
    }


def _parse_sample_times(value: str) -> list[float]:
    times = [float(part.strip()) for part in value.split(",") if part.strip()]
    if not times:
        raise ValueError("sample times must include at least one time")
    return times


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--transcript", required=True)
    parser.add_argument("--sample-times", required=True)
    parser.add_argument("--style", choices=sorted(SUPPORTED_STYLES), default="karaoke_fill")
    args = parser.parse_args()

    report = build_progress_report(
        json.loads(Path(args.transcript).read_text()),
        sample_times=_parse_sample_times(args.sample_times),
        style=args.style,
    )
    print(json.dumps(report, indent=2))
    if report["status"] != "ready":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
