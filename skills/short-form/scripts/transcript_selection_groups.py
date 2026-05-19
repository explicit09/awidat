#!/usr/bin/env python3
"""Build compact transcript selection packs from named groups and notes."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any, Iterable


TOKEN_RE = re.compile(r"[A-Za-z0-9']+")


def _float_value(item: dict[str, Any], *keys: str, default: float = 0.0) -> float:
    for key in keys:
        if key in item and item[key] is not None:
            return float(item[key])
    return default


def _tokens(value: str | None) -> set[str]:
    return {token.lower() for token in TOKEN_RE.findall(value or "")}


def _body(path: str) -> dict[str, Any]:
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def _segments(body: dict[str, Any]) -> list[dict[str, Any]]:
    normalized: list[dict[str, Any]] = []
    for segment in body.get("segments", []):
        start = _float_value(segment, "start_s", "start")
        end = _float_value(segment, "end_s", "end", default=start)
        text = str(segment.get("text", "")).strip()
        if not text:
            continue
        normalized.append({
            "start_s": round(start, 3),
            "end_s": round(max(start, end), 3),
            "speaker": segment.get("speaker_id", segment.get("speaker")),
            "text": text,
        })
    return sorted(normalized, key=lambda item: (item["start_s"], item["end_s"]))


def _intervals(raw_intervals: Iterable[dict[str, Any]]) -> list[dict[str, float]]:
    intervals: list[dict[str, float]] = []
    for interval in raw_intervals:
        start = _float_value(interval, "start_s", "start")
        end = _float_value(interval, "end_s", "end", default=start)
        if end <= start:
            continue
        intervals.append({"start_s": round(start, 3), "end_s": round(end, 3)})
    return sorted(intervals, key=lambda item: (item["start_s"], item["end_s"]))


def _merge_intervals(intervals: list[dict[str, float]]) -> list[dict[str, float]]:
    merged: list[dict[str, float]] = []
    for interval in intervals:
        if not merged or interval["start_s"] > merged[-1]["end_s"]:
            merged.append(dict(interval))
        else:
            merged[-1]["end_s"] = max(merged[-1]["end_s"], interval["end_s"])
    return merged


def _groups(body: dict[str, Any]) -> list[dict[str, Any]]:
    raw_groups = body.get("transcript_groups", body.get("groups", []))
    groups: list[dict[str, Any]] = []
    if isinstance(raw_groups, dict):
        iterable = raw_groups.items()
    else:
        iterable = ((str(index), group) for index, group in enumerate(raw_groups))

    for group_id, group in iterable:
        if not isinstance(group, dict):
            continue
        name = str(group.get("name", group.get("group_name", group_id))).strip()
        notes = str(group.get("notes", group.get("group_notes", ""))).strip()
        intervals = _intervals(group.get("time_intervals", group.get("intervals", [])))
        if name and intervals:
            groups.append({
                "group_id": str(group_id),
                "group_name": name,
                "notes": notes,
                "intervals": intervals,
            })
    return groups


def _overlaps(segment: dict[str, Any], interval: dict[str, float]) -> bool:
    return segment["start_s"] < interval["end_s"] and segment["end_s"] > interval["start_s"]


def _segments_for_group(
    segments: list[dict[str, Any]],
    intervals: list[dict[str, float]],
) -> list[dict[str, Any]]:
    selected: list[dict[str, Any]] = []
    seen: set[tuple[float, float, str]] = set()
    for segment in segments:
        if not any(_overlaps(segment, interval) for interval in intervals):
            continue
        key = (segment["start_s"], segment["end_s"], segment["text"])
        if key in seen:
            continue
        seen.add(key)
        selected.append(dict(segment))
    return selected


def _matches_group(
    group: dict[str, Any],
    selected_segments: list[dict[str, Any]],
    group_filters: list[str],
    query: str | None,
) -> bool:
    searchable = " ".join(
        [
            str(group["group_name"]),
            str(group.get("notes", "")),
            " ".join(str(segment["text"]) for segment in selected_segments),
        ]
    )
    searchable_lower = searchable.lower()
    if group_filters and not any(item.lower() in searchable_lower for item in group_filters):
        return False
    if query:
        query_tokens = _tokens(query)
        if not query_tokens:
            return True
        return query_tokens.issubset(_tokens(searchable))
    return True


def build_selection_pack(
    sources: list[tuple[str, dict[str, Any]]],
    *,
    group_filters: list[str],
    query: str | None,
) -> dict[str, Any]:
    """Return story-ready transcript selections from source sidecars."""
    output_groups: list[dict[str, Any]] = []
    warnings: list[str] = []

    for source_label, body in sources:
        source_segments = _segments(body)
        source_groups = _groups(body)
        if not source_groups:
            warnings.append(f"{source_label}: no transcript groups found")
            continue

        for group in source_groups:
            selected_segments = _segments_for_group(source_segments, group["intervals"])
            if not selected_segments:
                warnings.append(f"{source_label}: group {group['group_name']} has no matching segments")
                continue
            if not _matches_group(group, selected_segments, group_filters, query):
                continue

            merged = _merge_intervals(group["intervals"])
            text = " ".join(segment["text"] for segment in selected_segments).strip()
            duration = sum(interval["end_s"] - interval["start_s"] for interval in merged)
            output_groups.append({
                "source_label": source_label,
                "group_id": group["group_id"],
                "group_name": group["group_name"],
                "notes": group["notes"],
                "merged_intervals": merged,
                "duration_s": round(duration, 3),
                "text": text,
                "selected_segments": selected_segments,
            })

    status = "ready" if output_groups else "blocked"
    if status == "blocked" and not warnings:
        warnings.append("No transcript groups matched the requested filters")
    return {
        "status": status,
        "group_filters": group_filters,
        "query": query,
        "groups": output_groups,
        "warnings": warnings,
    }


def render_markdown(pack: dict[str, Any]) -> str:
    lines = ["# Transcript selection groups", ""]
    if pack.get("status") != "ready":
        lines.append("Status: blocked")
        for warning in pack.get("warnings", []):
            lines.append(f"- {warning}")
        return "\n".join(lines).rstrip() + "\n"

    for group in pack.get("groups", []):
        lines.append(f"## {group['source_label']} / {group['group_name']}")
        if group.get("notes"):
            lines.append(f"Notes: {group['notes']}")
        for interval in group.get("merged_intervals", []):
            lines.append(f"- [{interval['start_s']:.2f}-{interval['end_s']:.2f}]")
        if group.get("text"):
            lines.append(group["text"])
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--transcript", action="append", required=True)
    parser.add_argument("--source-label", action="append", default=[])
    parser.add_argument("--group", action="append", default=[])
    parser.add_argument("--query")
    parser.add_argument("--format", choices=["json", "markdown"], default="json")
    args = parser.parse_args()

    sources = []
    for index, transcript_path in enumerate(args.transcript):
        label = args.source_label[index] if index < len(args.source_label) else Path(transcript_path).stem
        sources.append((label, _body(transcript_path)))

    pack = build_selection_pack(sources, group_filters=args.group, query=args.query)
    if args.format == "markdown":
        print(render_markdown(pack), end="")
    else:
        print(json.dumps(pack, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
