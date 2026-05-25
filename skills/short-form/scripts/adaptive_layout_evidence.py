#!/usr/bin/env python3
"""Visual evidence normalization for adaptive layout planning."""

from __future__ import annotations

from statistics import mean
from typing import Any


def composition_for_time(
    composition: dict[str, Any] | None, start_s: float, end_s: float
) -> dict[str, Any]:
    if not composition:
        return {}
    best: tuple[float, dict[str, Any]] | None = None
    for region in composition.get("regions", []):
        region_start = float(region.get("start_s", region.get("start", start_s)))
        region_end = float(region.get("end_s", region.get("end", end_s)))
        overlap = max(0.0, min(end_s, region_end) - max(start_s, region_start))
        if overlap > 0.0 and (best is None or overlap > best[0]):
            best = (overlap, region)
    return dict(best[1]) if best else {}


def scene_id(region: dict[str, Any]) -> str:
    if not region:
        return "scene-unknown"
    start = float(region.get("start_s", region.get("start", 0.0)))
    end = float(region.get("end_s", region.get("end", start)))
    return f"scene-{start:.3f}-{end:.3f}"


def frame_quality_risk(
    frame_quality: dict[str, Any] | None, start_s: float, end_s: float
) -> float:
    if not frame_quality:
        return 0.0
    risks: list[float] = []
    for frame in frame_quality.get("per_frame", []):
        t_s = float(frame.get("t_s", 0.0))
        if not (start_s <= t_s <= end_s):
            continue
        blur = float(frame.get("blur", 100.0))
        brightness = float(frame.get("brightness", 128.0))
        contrast = float(frame.get("contrast", 40.0))
        blur_risk = max(0.0, min(1.0, (100.0 - blur) / 100.0))
        exposure_risk = max(0.0, min(1.0, abs(brightness - 128.0) / 128.0))
        contrast_risk = max(0.0, min(1.0, (32.0 - contrast) / 32.0))
        risks.append(max(blur_risk, exposure_risk, contrast_risk))
    return mean(risks) if risks else 0.0


def face_regions(
    face: dict[str, Any] | None,
    *,
    start_s: float,
    end_s: float,
    frame_width: int,
    frame_height: int,
) -> list[dict[str, Any]]:
    if not face:
        return []
    detect_width = float(face.get("detect_width", frame_width))
    detect_height = float(face.get("detect_height", frame_height))
    regions: list[dict[str, Any]] = []
    for entry in face.get("per_frame", []):
        t_s = float(entry.get("t_s", 0.0))
        if not (start_s <= t_s <= end_s):
            continue
        for face_item in entry.get("faces", []):
            face_px = _to_pixels(
                _face_box(face_item, detect_width=detect_width, detect_height=detect_height),
                frame_width=frame_width,
                frame_height=frame_height,
            )
            if face_px["width"] <= 0 or face_px["height"] <= 0:
                continue
            regions.extend(_derived_face_regions(face_px, face_item, frame_width=frame_width))
    return regions


def important_regions(
    region: dict[str, Any], *, frame_width: int, frame_height: int
) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for item in region.get("important_regions", []):
        box = item.get("box", {})
        if not isinstance(box, dict):
            continue
        out.append({
            "kind": str(item.get("kind", "key_action")),
            "box": {
                "x": float(box.get("x", 0.0)) * frame_width,
                "y": float(box.get("y", 0.0)) * frame_height,
                "width": float(box.get("width", 0.0)) * frame_width,
                "height": float(box.get("height", 0.0)) * frame_height,
            },
            "weight": float(item.get("weight", 1.0)),
        })
    return out


def _face_box(face: dict[str, Any], *, detect_width: float, detect_height: float) -> dict[str, float]:
    top, right, bottom, left = [float(value) for value in face.get("box", [0, 0, 0, 0])]
    scale_x = 1.0 if detect_width <= 0 else 1.0 / detect_width
    scale_y = 1.0 if detect_height <= 0 else 1.0 / detect_height
    return {
        "x": left * scale_x,
        "y": top * scale_y,
        "width": max(0.0, (right - left) * scale_x),
        "height": max(0.0, (bottom - top) * scale_y),
    }


def _to_pixels(box: dict[str, float], *, frame_width: int, frame_height: int) -> dict[str, float]:
    return {
        "x": box["x"] * frame_width,
        "y": box["y"] * frame_height,
        "width": box["width"] * frame_width,
        "height": box["height"] * frame_height,
    }


def _inflate(box: dict[str, float], amount_x: float, amount_y: float) -> dict[str, float]:
    return {
        "x": box["x"] - amount_x,
        "y": box["y"] - amount_y,
        "width": box["width"] + amount_x * 2.0,
        "height": box["height"] + amount_y * 2.0,
    }


def _derived_face_regions(
    face_px: dict[str, float], face_item: dict[str, Any], *, frame_width: int
) -> list[dict[str, Any]]:
    eye_box = {
        "x": face_px["x"],
        "y": face_px["y"],
        "width": face_px["width"],
        "height": face_px["height"] * 0.38,
    }
    mouth_box = {
        "x": face_px["x"] + face_px["width"] * 0.12,
        "y": face_px["y"] + face_px["height"] * 0.58,
        "width": face_px["width"] * 0.76,
        "height": face_px["height"] * 0.34,
    }
    gaze_box = _inflate(face_px, face_px["width"] * 0.25, face_px["height"] * 0.2)
    gaze_box["x"] += float(face_item.get("gaze_score", 0.0)) * frame_width * 0.16
    return [
        {"kind": "face", "box": _inflate(face_px, 18.0, 18.0), "weight": 1.0},
        {"kind": "eyes", "box": _inflate(eye_box, 24.0, 16.0), "weight": 1.0},
        {"kind": "mouth", "box": _inflate(mouth_box, 24.0, 16.0), "weight": 1.0},
        {"kind": "subject_body", "box": _inflate(face_px, face_px["width"] * 0.35, face_px["height"] * 0.95), "weight": 0.45},
        {"kind": "gaze_line", "box": gaze_box, "weight": 0.35},
    ]
