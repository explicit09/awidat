#!/usr/bin/env python3
"""Group word-level transcript timing into short caption phrases."""

from __future__ import annotations

import argparse
import json
import textwrap
from pathlib import Path

from transcript_phrases import group_words_into_phrases, normalize_words


CAPTION_STYLES = {
    "classic": {
        "position": "bottom",
        "font_size": 56,
        "color": "#FFFFFF",
        "font_weight": "normal",
        "animation": "fade_in_out",
        "background": "rgba(0,0,0,0.64)",
        "stroke_width": 3,
        "safe_area": "mobile",
        "z_index": 80,
    },
    "impact": {
        "position": "center",
        "font_size": 64,
        "color": "#FFFFFF",
        "font_weight": "bold",
        "animation": "pop_in",
        "background": "transparent",
        "stroke_width": 5,
        "safe_area": "mobile",
        "z_index": 80,
    },
    "boxed": {
        "position": "bottom",
        "font_size": 52,
        "color": "#FFFFFF",
        "font_weight": "bold",
        "animation": "reveal",
        "background": "rgba(0,0,0,0.72)",
        "stroke_width": 2,
        "safe_area": "mobile",
        "z_index": 80,
    },
    "minimal": {
        "position": "bottom",
        "font_size": 48,
        "color": "#FFFFFF",
        "font_weight": "normal",
        "animation": "none",
        "background": "rgba(0,0,0,0.48)",
        "stroke_width": 2,
        "safe_area": "mobile",
        "z_index": 80,
    },
}

READABILITY_DEFAULTS = {
    "max_cps": 24.0,
    "min_duration_s": 0.6,
    "max_duration_s": 6.0,
    "max_chars_per_line": 32,
    "max_lines": 2,
}

GEOMETRY_DEFAULTS = {
    "frame_width": 1080,
    "frame_height": 1920,
    "safe_margin_x": 72,
    "safe_margin_y": 144,
    "min_stroke_width": 2,
    "overlay_z_index": 40,
}


def _format_subtitle_timestamp(seconds: float, *, millisecond_separator: str) -> str:
    if seconds < 0 or not isinstance(seconds, int | float):
        raise ValueError("subtitle timestamp seconds must be a non-negative number")
    milliseconds = round(float(seconds) * 1000.0)
    hours = milliseconds // 3_600_000
    milliseconds -= hours * 3_600_000
    minutes = milliseconds // 60_000
    milliseconds -= minutes * 60_000
    whole_seconds = milliseconds // 1_000
    milliseconds -= whole_seconds * 1_000
    return (
        f"{hours:02d}:{minutes:02d}:{whole_seconds:02d}"
        f"{millisecond_separator}{milliseconds:03d}"
    )


def format_srt_timestamp(seconds: float) -> str:
    return _format_subtitle_timestamp(seconds, millisecond_separator=",")


def format_vtt_timestamp(seconds: float) -> str:
    return _format_subtitle_timestamp(seconds, millisecond_separator=".")


def _build_subtitle_cues(phrases: list[dict], *, timestamp_formatter) -> str:
    cues = []
    previous_start = -1.0
    for index, phrase in enumerate(phrases, start=1):
        text = str(phrase.get("text", "")).strip().replace("-->", "->")
        start = float(phrase.get("start_s", 0.0))
        end = float(phrase.get("end_s", start))
        if not text:
            raise ValueError(f"subtitle cue {index} text is empty")
        if start < 0.0:
            raise ValueError(f"subtitle cue {index} start_s must be non-negative")
        if end <= start:
            raise ValueError(f"subtitle cue {index} end_s must be greater than start_s")
        if start < previous_start:
            raise ValueError("subtitle cues must be sorted by start_s")
        previous_start = start
        cues.append(
            "\n".join([
                str(index),
                f"{timestamp_formatter(start)} --> {timestamp_formatter(end)}",
                text,
            ])
        )
    return "\n\n".join(cues) + ("\n" if cues else "")


def build_srt(phrases: list[dict]) -> str:
    return _build_subtitle_cues(phrases, timestamp_formatter=format_srt_timestamp)


def build_vtt(phrases: list[dict]) -> str:
    return "WEBVTT\n\n" + _build_subtitle_cues(
        phrases,
        timestamp_formatter=format_vtt_timestamp,
    )


def wrap_caption_text(text: str, *, max_chars_per_line: int | None = None) -> str:
    normalized = " ".join(str(text).strip().split())
    if max_chars_per_line is None:
        return normalized
    if max_chars_per_line < 1:
        raise ValueError("max_chars_per_line must be positive")
    return textwrap.fill(
        normalized,
        width=max_chars_per_line,
        break_long_words=False,
        break_on_hyphens=False,
    )


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
    phrase_preset: str | None = None,
    hot_start_s: float | None = None,
    hot_end_s: float | None = None,
    max_chars_per_line: int | None = None,
    style: str = "classic",
) -> list[dict]:
    phrases = []
    preset = CAPTION_STYLES.get(style, CAPTION_STYLES["classic"])
    for phrase in group_words_into_phrases(
        items,
        max_words=max_words,
        max_gap_s=max_gap_s,
        phrase_preset=phrase_preset,
    ):
        start = float(phrase.get("start_s", 0.0))
        end = float(phrase.get("end_s", start))
        hot = (
            hot_start_s is not None
            and hot_end_s is not None
            and end >= hot_start_s
            and start <= hot_end_s
        )
        styled = dict(preset)
        styled["style"] = style if style in CAPTION_STYLES else "classic"
        if hot:
            styled["color"] = "#FFD400"
            styled["font_weight"] = "bold"
        phrases.append({
            "text": wrap_caption_text(
                str(phrase.get("text", "")),
                max_chars_per_line=max_chars_per_line,
            ),
            "start_s": round(start, 3),
            "end_s": round(max(end, start + 0.6), 3),
            "word_timings": phrase.get("word_timings", []),
            **styled,
        })
    return phrases


def _caption_lines(text: object) -> list[str]:
    return str(text or "").strip().splitlines() or [str(text or "").strip()]


def build_readability_scorecard(
    phrases: list[dict],
    *,
    max_cps: float = READABILITY_DEFAULTS["max_cps"],
    min_duration_s: float = READABILITY_DEFAULTS["min_duration_s"],
    max_duration_s: float = READABILITY_DEFAULTS["max_duration_s"],
    max_chars_per_line: int = READABILITY_DEFAULTS["max_chars_per_line"],
    max_lines: int = READABILITY_DEFAULTS["max_lines"],
) -> dict:
    if max_cps <= 0:
        raise ValueError("max_cps must be positive")
    if min_duration_s <= 0 or max_duration_s < min_duration_s:
        raise ValueError("duration bounds must be positive and ordered")
    if max_chars_per_line < 1:
        raise ValueError("max_chars_per_line must be positive")
    if max_lines < 1:
        raise ValueError("max_lines must be positive")

    cue_reports = []
    all_issues = []
    max_observed_cps = 0.0
    max_observed_line = 0
    max_observed_lines = 0
    for index, phrase in enumerate(phrases, start=1):
        text = str(phrase.get("text", "")).strip()
        start = float(phrase.get("start_s", 0.0))
        end = float(phrase.get("end_s", start))
        duration = max(0.0, end - start)
        lines = _caption_lines(text)
        plain_char_count = len("".join(lines))
        cps = plain_char_count / duration if duration > 0 else float("inf")
        longest_line = max((len(line) for line in lines), default=0)
        max_observed_cps = max(max_observed_cps, cps)
        max_observed_line = max(max_observed_line, longest_line)
        max_observed_lines = max(max_observed_lines, len(lines))
        issues = []

        def add_issue(code: str, message: str) -> None:
            issue = {"cue": index, "code": code, "message": message}
            issues.append(issue)
            all_issues.append(issue)

        if duration < min_duration_s:
            add_issue("duration_too_short", f"cue duration {duration:.3f}s is below {min_duration_s:.3f}s")
        if duration > max_duration_s:
            add_issue("duration_too_long", f"cue duration {duration:.3f}s is above {max_duration_s:.3f}s")
        if cps > max_cps:
            add_issue("cps_too_high", f"cue CPS {cps:.2f} is above {max_cps:.2f}")
        if longest_line > max_chars_per_line:
            add_issue(
                "line_too_long",
                f"longest line {longest_line} chars is above {max_chars_per_line}",
            )
        if len(lines) > max_lines:
            add_issue("too_many_lines", f"cue has {len(lines)} lines; max is {max_lines}")

        cue_reports.append({
            "index": index,
            "text": text,
            "start_s": round(start, 3),
            "end_s": round(end, 3),
            "duration_s": round(duration, 3),
            "char_count": plain_char_count,
            "line_count": len(lines),
            "longest_line_chars": longest_line,
            "cps": round(cps, 3) if cps != float("inf") else cps,
            "status": "needs_review" if issues else "ready",
            "issues": issues,
        })

    return {
        "version": 1,
        "status": "needs_review" if all_issues else "ready",
        "cue_count": len(cue_reports),
        "limits": {
            "max_cps": max_cps,
            "min_duration_s": min_duration_s,
            "max_duration_s": max_duration_s,
            "max_chars_per_line": max_chars_per_line,
            "max_lines": max_lines,
        },
        "max_cps": round(max_observed_cps, 3),
        "max_line_chars": max_observed_line,
        "max_line_count": max_observed_lines,
        "issue_count": len(all_issues),
        "issues": all_issues,
        "cues": cue_reports,
    }


def _caption_box(
    phrase: dict,
    *,
    frame_width: int,
    frame_height: int,
    safe_margin_y: int,
) -> dict:
    text = str(phrase.get("text", "")).strip()
    lines = _caption_lines(text)
    font_size = float(phrase.get("font_size", CAPTION_STYLES["classic"]["font_size"]))
    longest_line = max((len(line) for line in lines), default=0)
    width = int(round(longest_line * font_size * 0.58 + font_size * 0.72))
    height = int(round(len(lines) * font_size * 1.2 + font_size * 0.64))
    x = int(round((frame_width - width) / 2))
    position = str(phrase.get("position", "bottom"))
    if position == "top":
        y = safe_margin_y
    elif position == "center":
        y = int(round((frame_height - height) / 2))
    else:
        y = frame_height - safe_margin_y - height
    return {"x": x, "y": y, "width": width, "height": height}


def _inside_safe_area(
    box: dict,
    *,
    frame_width: int,
    frame_height: int,
    safe_margin_x: int,
    safe_margin_y: int,
) -> bool:
    return (
        box["x"] >= safe_margin_x
        and box["y"] >= safe_margin_y
        and box["x"] + box["width"] <= frame_width - safe_margin_x
        and box["y"] + box["height"] <= frame_height - safe_margin_y
    )


def _has_visible_caption_backing(phrase: dict, *, min_stroke_width: int) -> bool:
    background = str(phrase.get("background", "")).strip().lower()
    stroke_width = float(phrase.get("stroke_width", 0))
    return stroke_width >= min_stroke_width or background not in {"", "transparent", "none"}


def build_geometry_scorecard(
    phrases: list[dict],
    *,
    frame_width: int = GEOMETRY_DEFAULTS["frame_width"],
    frame_height: int = GEOMETRY_DEFAULTS["frame_height"],
    safe_margin_x: int = GEOMETRY_DEFAULTS["safe_margin_x"],
    safe_margin_y: int = GEOMETRY_DEFAULTS["safe_margin_y"],
    min_stroke_width: int = GEOMETRY_DEFAULTS["min_stroke_width"],
    overlay_z_index: int = GEOMETRY_DEFAULTS["overlay_z_index"],
) -> dict:
    if frame_width <= 0 or frame_height <= 0:
        raise ValueError("frame dimensions must be positive")
    if safe_margin_x < 0 or safe_margin_y < 0:
        raise ValueError("safe margins must be non-negative")
    if min_stroke_width < 0:
        raise ValueError("min_stroke_width must be non-negative")

    cue_reports = []
    all_issues = []
    for index, phrase in enumerate(phrases, start=1):
        box = _caption_box(
            phrase,
            frame_width=frame_width,
            frame_height=frame_height,
            safe_margin_y=safe_margin_y,
        )
        inside_safe_area = _inside_safe_area(
            box,
            frame_width=frame_width,
            frame_height=frame_height,
            safe_margin_x=safe_margin_x,
            safe_margin_y=safe_margin_y,
        )
        has_backing = _has_visible_caption_backing(
            phrase,
            min_stroke_width=min_stroke_width,
        )
        z_index = int(phrase.get("z_index", 0))
        issues = []

        def add_issue(code: str, message: str) -> None:
            issue = {"cue": index, "code": code, "message": message}
            issues.append(issue)
            all_issues.append(issue)

        if not inside_safe_area:
            add_issue("outside_safe_area", "estimated caption box exceeds safe-area bounds")
        if not has_backing:
            add_issue(
                "missing_contrast_support",
                "caption needs a non-transparent background or sufficient stroke width",
            )
        if z_index <= overlay_z_index:
            add_issue(
                "caption_below_overlay",
                f"caption z_index {z_index} must be above overlay z_index {overlay_z_index}",
            )

        cue_reports.append({
            "index": index,
            "text": str(phrase.get("text", "")).strip(),
            "position": str(phrase.get("position", "bottom")),
            "font_size": float(phrase.get("font_size", CAPTION_STYLES["classic"]["font_size"])),
            "background": str(phrase.get("background", "")),
            "stroke_width": float(phrase.get("stroke_width", 0)),
            "z_index": z_index,
            "box": box,
            "inside_safe_area": inside_safe_area,
            "has_contrast_support": has_backing,
            "status": "needs_review" if issues else "ready",
            "issues": issues,
        })

    return {
        "version": 1,
        "status": "needs_review" if all_issues else "ready",
        "cue_count": len(cue_reports),
        "frame": {
            "width": frame_width,
            "height": frame_height,
            "safe_margin_x": safe_margin_x,
            "safe_margin_y": safe_margin_y,
            "overlay_z_index": overlay_z_index,
        },
        "limits": {"min_stroke_width": min_stroke_width},
        "issue_count": len(all_issues),
        "issues": all_issues,
        "cues": cue_reports,
    }


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--transcript", required=True)
    p.add_argument("--max-words", type=int, default=4)
    p.add_argument("--max-gap-s", type=float, default=0.5)
    p.add_argument("--phrase-preset", choices=("short", "medium", "long"))
    p.add_argument("--hot-start-s", type=float)
    p.add_argument("--hot-end-s", type=float)
    p.add_argument("--max-chars-per-line", type=int)
    p.add_argument("--style", choices=sorted(CAPTION_STYLES), default="classic")
    p.add_argument("--readability-max-cps", type=float, default=READABILITY_DEFAULTS["max_cps"])
    p.add_argument(
        "--readability-min-duration-s",
        type=float,
        default=READABILITY_DEFAULTS["min_duration_s"],
    )
    p.add_argument(
        "--readability-max-duration-s",
        type=float,
        default=READABILITY_DEFAULTS["max_duration_s"],
    )
    p.add_argument(
        "--readability-max-lines",
        type=int,
        default=READABILITY_DEFAULTS["max_lines"],
    )
    p.add_argument("--geometry-frame-width", type=int, default=GEOMETRY_DEFAULTS["frame_width"])
    p.add_argument("--geometry-frame-height", type=int, default=GEOMETRY_DEFAULTS["frame_height"])
    p.add_argument("--geometry-safe-margin-x", type=int, default=GEOMETRY_DEFAULTS["safe_margin_x"])
    p.add_argument("--geometry-safe-margin-y", type=int, default=GEOMETRY_DEFAULTS["safe_margin_y"])
    p.add_argument("--geometry-overlay-z-index", type=int, default=GEOMETRY_DEFAULTS["overlay_z_index"])
    p.add_argument(
        "--output-format",
        choices=("json", "srt", "vtt", "scorecard", "geometry-scorecard"),
        default="json",
    )
    args = p.parse_args()

    items = words(load_body(args.transcript))
    phrases = build_caption_phrases(
        items,
        max_words=args.max_words,
        max_gap_s=args.max_gap_s,
        phrase_preset=args.phrase_preset,
        hot_start_s=args.hot_start_s,
        hot_end_s=args.hot_end_s,
        max_chars_per_line=args.max_chars_per_line,
        style=args.style,
    )
    if args.output_format == "srt":
        print(build_srt(phrases), end="")
    elif args.output_format == "vtt":
        print(build_vtt(phrases), end="")
    elif args.output_format == "scorecard":
        print(json.dumps(
            build_readability_scorecard(
                phrases,
                max_cps=args.readability_max_cps,
                min_duration_s=args.readability_min_duration_s,
                max_duration_s=args.readability_max_duration_s,
                max_chars_per_line=args.max_chars_per_line
                or READABILITY_DEFAULTS["max_chars_per_line"],
                max_lines=args.readability_max_lines,
            ),
            indent=2,
        ))
    elif args.output_format == "geometry-scorecard":
        print(json.dumps(
            build_geometry_scorecard(
                phrases,
                frame_width=args.geometry_frame_width,
                frame_height=args.geometry_frame_height,
                safe_margin_x=args.geometry_safe_margin_x,
                safe_margin_y=args.geometry_safe_margin_y,
                overlay_z_index=args.geometry_overlay_z_index,
            ),
            indent=2,
        ))
    else:
        print(json.dumps({"phrases": phrases}, indent=2))


if __name__ == "__main__":
    main()
