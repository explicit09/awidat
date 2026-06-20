#!/usr/bin/env python3
"""Plan dynamic vertical layouts for two-person short-form clips."""

from __future__ import annotations

import argparse
import json
import re
from typing import Any


def _speaker_key(value: object) -> str:
    text = str(value or "").strip()
    if not text:
        return "unknown"
    match = re.search(r"\d+", text)
    return match.group(0) if match else text


def _segment_speaker(segment: dict[str, Any]) -> str | None:
    speaker = segment.get("speaker_id", segment.get("speaker"))
    if speaker is None or not str(speaker).strip():
        return None
    return _speaker_key(speaker)


def _segment_start(segment: dict[str, Any]) -> float:
    return float(segment.get("start_s", segment.get("start", 0.0)))


def _segment_end(segment: dict[str, Any]) -> float:
    return float(segment.get("end_s", segment.get("end", _segment_start(segment))))


def normalize_speaker_segments(body: dict[str, Any]) -> list[dict[str, Any]]:
    data = body.get("data", body)
    out = []
    for segment in data.get("segments", []):
        start = _segment_start(segment)
        end = _segment_end(segment)
        speaker = _segment_speaker(segment)
        if end <= start:
            continue
        if speaker is None:
            continue
        out.append({
            "start_s": start,
            "end_s": end,
            "speaker": speaker,
        })
    return sorted(out, key=lambda item: item["start_s"])


def _merge_same_speaker_turns(
    segments: list[dict[str, Any]],
    *,
    max_gap_s: float,
) -> list[dict[str, Any]]:
    turns: list[dict[str, Any]] = []
    for segment in segments:
        if (
            turns
            and turns[-1]["speaker"] == segment["speaker"]
            and segment["start_s"] - turns[-1]["end_s"] <= max_gap_s
        ):
            turns[-1]["end_s"] = max(turns[-1]["end_s"], segment["end_s"])
        else:
            turns.append(dict(segment))
    return turns


def _other_speaker(active: str, speaker_order: list[str]) -> str | None:
    for speaker in speaker_order:
        if speaker != active:
            return speaker
    return None


def _normalize_slot_evidence(
    speaker_to_slot: dict[str, str] | None,
    speaker_slot_evidence: dict[str, Any] | None,
) -> dict[str, dict[str, Any]]:
    evidence: dict[str, dict[str, Any]] = {}
    for speaker, value in (speaker_slot_evidence or {}).items():
        key = _speaker_key(speaker)
        if isinstance(value, dict):
            evidence[key] = {
                "slot": value.get("slot"),
                "confidence": float(value.get("confidence", 0.0)),
                "method": value.get("method", "unknown"),
            }
        else:
            evidence[key] = {
                "slot": str(value),
                "confidence": 0.5,
                "method": "legacy_mapping",
            }
    for speaker, slot in (speaker_to_slot or {}).items():
        key = _speaker_key(speaker)
        evidence.setdefault(
            key,
            {"slot": str(slot), "confidence": 0.5, "method": "manual_or_legacy_mapping"},
        )
    return evidence


def _slot_for(evidence: dict[str, dict[str, Any]], speaker: str | None) -> str | None:
    if speaker is None:
        return None
    slot = evidence.get(speaker, {}).get("slot")
    return str(slot) if slot is not None else None


def _speaker_durations(turns: list[dict[str, Any]]) -> dict[str, float]:
    durations: dict[str, float] = {}
    for turn in turns:
        durations[turn["speaker"]] = durations.get(turn["speaker"], 0.0) + (
            turn["end_s"] - turn["start_s"]
        )
    return durations


def _append_layout(
    layouts: list[dict[str, Any]],
    layout: dict[str, Any],
    *,
    min_layout_s: float,
) -> None:
    if not layouts:
        layouts.append(layout)
        return
    previous = layouts[-1]
    if previous["mode"] == layout["mode"] and previous.get("active_speaker") == layout.get("active_speaker") and previous.get("top_speaker") == layout.get("top_speaker"):
        previous["end_s"] = max(previous["end_s"], layout["end_s"])
        return
    if layout["end_s"] - layout["start_s"] < min_layout_s:
        previous["end_s"] = max(previous["end_s"], layout["end_s"])
        previous.setdefault("reasons", []).append("absorbed_short_layout")
        return
    layouts.append(layout)


def _split_layout(start_s: float, end_s: float, reason: str) -> dict[str, Any]:
    return {
        "start_s": round(start_s, 3),
        "end_s": round(end_s, 3),
        "mode": "split_stacked",
        "reason": reason,
    }


def plan_short_form_layout(
    segments: list[dict[str, Any]],
    *,
    clip_start_s: float,
    clip_end_s: float,
    speaker_to_slot: dict[str, str] | None = None,
    speaker_slot_evidence: dict[str, Any] | None = None,
    min_fill_s: float = 8.0,
    min_layout_s: float = 3.0,
    max_same_speaker_gap_s: float = 2.0,
    dominant_fill_ratio: float = 0.8,
    min_slot_confidence: float = 0.65,
) -> dict[str, Any]:
    if clip_end_s <= clip_start_s:
        raise ValueError("clip_end_s must be greater than clip_start_s")
    relevant = [
        {
            "start_s": max(clip_start_s, item["start_s"]) - clip_start_s,
            "end_s": min(clip_end_s, item["end_s"]) - clip_start_s,
            "speaker": item["speaker"],
        }
        for item in segments
        if item["end_s"] > clip_start_s and item["start_s"] < clip_end_s
    ]
    relevant = [item for item in relevant if item["end_s"] > item["start_s"]]
    if not relevant:
        return {
            "version": 1,
            "status": "needs_review",
            "layouts": [{"start_s": 0.0, "end_s": round(clip_end_s - clip_start_s, 3), "mode": "split_stacked"}],
            "warnings": ["no_speaker_segments"],
        }
    speaker_order = []
    for item in relevant:
        if item["speaker"] not in speaker_order:
            speaker_order.append(item["speaker"])
    turns = _merge_same_speaker_turns(relevant, max_gap_s=max_same_speaker_gap_s)
    slot_evidence = _normalize_slot_evidence(speaker_to_slot, speaker_slot_evidence)
    warnings = []
    low_confidence = [
        speaker
        for speaker in speaker_order
        if speaker not in slot_evidence
        or float(slot_evidence[speaker].get("confidence", 0.0)) < min_slot_confidence
    ]
    if low_confidence:
        warnings.append("speaker_slot_mapping_needs_visual_verification")

    durations = _speaker_durations(turns)
    spoken_duration = sum(durations.values())
    if spoken_duration > 0:
        dominant_speaker, dominant_duration = max(durations.items(), key=lambda item: item[1])
        if dominant_duration / spoken_duration >= dominant_fill_ratio:
            return {
                "version": 2,
                "status": "needs_review" if warnings else "ready",
                "clip_start_s": round(clip_start_s, 3),
                "clip_end_s": round(clip_end_s, 3),
                "speaker_order": speaker_order,
                "speaker_slot_evidence": slot_evidence,
                "layouts": [{
                    "start_s": 0.0,
                    "end_s": round(clip_end_s - clip_start_s, 3),
                    "mode": "fill",
                    "active_speaker": dominant_speaker,
                    "active_slot": _slot_for(slot_evidence, dominant_speaker),
                    "reason": "one speaker owns at least 80 percent of spoken time",
                }],
                "warnings": warnings,
                "verification_required": [
                    "confirm speaker-to-slot mapping against frames before render",
                    "check fill crop keeps the active speaker face in safe area",
                ],
            }

    layouts: list[dict[str, Any]] = []
    cursor_s = 0.0
    for turn in turns:
        if turn["start_s"] > cursor_s:
            _append_layout(
                layouts,
                _split_layout(cursor_s, turn["start_s"], "non-speech gap keeps both people visible"),
                min_layout_s=min_layout_s,
            )
        duration = turn["end_s"] - turn["start_s"]
        active = turn["speaker"]
        other = _other_speaker(active, speaker_order)
        if duration >= min_fill_s:
            layout = {
                "start_s": round(turn["start_s"], 3),
                "end_s": round(turn["end_s"], 3),
                "mode": "fill",
                "active_speaker": active,
                "active_slot": _slot_for(slot_evidence, active),
                "reason": "same speaker owns an extended turn",
            }
        else:
            layout = {
                "start_s": round(turn["start_s"], 3),
                "end_s": round(turn["end_s"], 3),
                "mode": "split_stacked",
                "top_speaker": active,
                "bottom_speaker": other,
                "top_slot": _slot_for(slot_evidence, active),
                "bottom_slot": _slot_for(slot_evidence, other),
                "reason": "dialogue or short turn keeps both people visible with active speaker on top",
            }
        _append_layout(layouts, layout, min_layout_s=min_layout_s)
        cursor_s = turn["end_s"]
    clip_duration_s = clip_end_s - clip_start_s
    if cursor_s < clip_duration_s:
        _append_layout(
            layouts,
            _split_layout(cursor_s, clip_duration_s, "tail gap keeps both people visible"),
            min_layout_s=min_layout_s,
        )
    return {
        "version": 2,
        "status": "needs_review" if warnings else "ready",
        "clip_start_s": round(clip_start_s, 3),
        "clip_end_s": round(clip_end_s, 3),
        "speaker_order": speaker_order,
        "speaker_slot_evidence": slot_evidence,
        "layouts": layouts,
        "warnings": warnings,
        "verification_required": [
            "confirm speaker-to-slot mapping against frames before render",
            "check split/fill transitions do not collide with captions",
        ],
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--transcript", required=True)
    parser.add_argument("--clip-start-s", type=float, required=True)
    parser.add_argument("--clip-end-s", type=float, required=True)
    parser.add_argument("--speaker-to-slot-json", default="{}")
    parser.add_argument("--speaker-slot-evidence-json", default="{}")
    parser.add_argument("--min-fill-s", type=float, default=8.0)
    parser.add_argument("--min-layout-s", type=float, default=3.0)
    parser.add_argument("--dominant-fill-ratio", type=float, default=0.8)
    args = parser.parse_args()
    body = json.loads(open(args.transcript).read())
    speaker_to_slot = json.loads(args.speaker_to_slot_json)
    speaker_slot_evidence = json.loads(args.speaker_slot_evidence_json)
    print(json.dumps(
        plan_short_form_layout(
            normalize_speaker_segments(body),
            clip_start_s=args.clip_start_s,
            clip_end_s=args.clip_end_s,
            speaker_to_slot={_speaker_key(k): str(v) for k, v in speaker_to_slot.items()},
            speaker_slot_evidence=speaker_slot_evidence,
            min_fill_s=args.min_fill_s,
            min_layout_s=args.min_layout_s,
            dominant_fill_ratio=args.dominant_fill_ratio,
        ),
        indent=2,
    ))


if __name__ == "__main__":
    main()
