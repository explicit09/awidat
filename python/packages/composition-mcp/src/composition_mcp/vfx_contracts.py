"""Lightweight VFX producer contracts.

These helpers do not run trackers or segmentation models. They validate
request-shaped inputs and emit dictionaries that match Awidat's
`TrackingPackage` sidecar shape closely enough for tests, planning, and
future producer integration.
"""

from __future__ import annotations

from typing import Any


def motion_track_package(
    *,
    asset_id: str,
    track_id: str,
    initial_box_xyxy: list[float],
    start_frame: int,
    end_frame: int,
    confidence: float,
) -> dict[str, Any]:
    _require_non_empty(asset_id, "asset_id")
    _require_non_empty(track_id, "track_id")
    _validate_frame_range(start_frame, end_frame)
    _validate_unit_interval(confidence, "confidence")
    x0, y0, x1, y1 = _validate_box(initial_box_xyxy)

    return {
        "tracks": [
            {
                "id": track_id,
                "asset_id": asset_id,
                "kind": "surface",
                "coordinate_space": "normalized",
                "samples": [
                    {
                        "frame": start_frame,
                        "points": [[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
                        "confidence": confidence,
                    }
                ],
                "confidence": confidence,
            }
        ],
        "producer": {
            "kind": "motion_tracker",
            "backend": "opencv.tracker",
            "status": "planned",
            "start_frame": start_frame,
            "end_frame": end_frame,
        },
    }


def mask_artifact_profile(
    *,
    artifact_id: str,
    path: str,
    object_ids: list[str],
    start_frame: int,
    end_frame: int,
    width: int,
    height: int,
    rate: float = 1.0,
) -> dict[str, Any]:
    _require_non_empty(artifact_id, "artifact_id")
    _require_non_empty(path, "path")
    if not object_ids or any(not str(object_id).strip() for object_id in object_ids):
        raise ValueError("object_ids must contain at least one non-empty id")
    _validate_frame_range(start_frame, end_frame)
    if width <= 0:
        raise ValueError("width must be > 0")
    if height <= 0:
        raise ValueError("height must be > 0")
    if rate <= 0.0:
        raise ValueError("rate must be > 0")

    frame_count = end_frame - start_frame + 1
    return {
        "id": artifact_id,
        "kind": "per_object_png",
        "path": path,
        "object_ids": object_ids,
        "frame_range": {
            "start_time": {"value": start_frame, "rate": rate},
            "duration": {"value": frame_count, "rate": rate},
        },
        "frame_count": frame_count,
        "width": width,
        "height": height,
    }


def _require_non_empty(value: str, label: str) -> None:
    if not str(value).strip():
        raise ValueError(f"{label} must be non-empty")


def _validate_frame_range(start_frame: int, end_frame: int) -> None:
    if start_frame < 0:
        raise ValueError("start_frame must be >= 0")
    if end_frame < start_frame:
        raise ValueError("end_frame must be >= start_frame")


def _validate_unit_interval(value: float, label: str) -> None:
    if value < 0.0 or value > 1.0:
        raise ValueError(f"{label} must be in 0..=1")


def _validate_box(box: list[float]) -> tuple[float, float, float, float]:
    if len(box) != 4:
        raise ValueError("box must have four coordinates")
    x0, y0, x1, y1 = (float(value) for value in box)
    for value in [x0, y0, x1, y1]:
        _validate_unit_interval(value, "box coordinate")
    if x1 <= x0 or y1 <= y0:
        raise ValueError("box must be ordered as x0 < x1 and y0 < y1")
    return x0, y0, x1, y1
