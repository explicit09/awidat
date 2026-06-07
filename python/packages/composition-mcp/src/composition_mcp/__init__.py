"""Composition indexer.

Emits shot-window composition regions that `shot-mcp` can merge into
its sidecar. This first implementation is lightweight and fail-soft:
it derives model-style labels from scenedetect plus optional face/gaze
sidecars, producing the same schema a heavier model-backed composition
indexer can replace later.

Schema version: "1".
"""

from __future__ import annotations

import json
import logging
from pathlib import Path
from statistics import mean
from typing import Any

try:
    from awidat_mcp import IndexAssetRequest, IndexerServer
except ModuleNotFoundError:
    IndexAssetRequest = Any
    IndexerServer = None

INDEXER_NAME = "composition"
INDEXER_VERSION = "0.1.0"
SCHEMA_VERSION = "1"

_log = logging.getLogger(INDEXER_NAME)

server = (
    IndexerServer(
        name=INDEXER_NAME,
        indexer_version=INDEXER_VERSION,
        schema_version=SCHEMA_VERSION,
    )
    if IndexerServer
    else None
)


def _project_root_from(req: IndexAssetRequest) -> Path:
    if req.project_root:
        return Path(req.project_root).absolute()
    p = Path(req.asset_path).absolute()
    while p != p.parent:
        if (p / "index").is_dir():
            return p
        p = p.parent
    return Path(req.asset_path).absolute().parent


def _read_sidecar(project_root: Path, indexer: str, asset_id: str) -> dict[str, Any] | None:
    path = project_root / "index" / indexer / f"{asset_id}.json"
    if not path.exists():
        return None
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return None


def _face_ratios_in_window(
    per_frame: list[dict[str, Any]],
    detect_w: int,
    detect_h: int,
    start_s: float,
    end_s: float,
) -> list[float]:
    frame_area = detect_w * detect_h
    if frame_area <= 0:
        return []
    ratios: list[float] = []
    for entry in per_frame:
        t_s = float(entry.get("t_s", 0.0))
        if not (start_s <= t_s < end_s):
            continue
        largest = 0.0
        for face in entry.get("faces", []):
            top, right, bottom, left = face.get("box", [0, 0, 0, 0])
            largest = max(largest, float(max(0, bottom - top) * max(0, right - left)))
        ratios.append(largest / frame_area)
    return ratios


def _gaze_in_window(
    per_frame: list[dict[str, Any]],
    start_s: float,
    end_s: float,
) -> list[float]:
    scores: list[float] = []
    for entry in per_frame:
        t_s = float(entry.get("t_s", 0.0))
        if not (start_s <= t_s < end_s):
            continue
        for face in entry.get("faces", []):
            if "gaze_score" in face:
                scores.append(float(face["gaze_score"]))
                break
    return scores


def _subject_role(face_ratio: float, gaze_scores: list[float]) -> str:
    if face_ratio <= 0.0:
        return "environment"
    if face_ratio < 0.025:
        return "background_person"
    if gaze_scores and abs(mean(gaze_scores)) <= 0.12:
        return "primary_speaker"
    return "featured_subject"


def _depth_layer(face_ratio: float) -> str:
    if face_ratio >= 0.10:
        return "foreground"
    if face_ratio >= 0.025:
        return "midground"
    return "background"


def _framing(face_ratio: float) -> str:
    if face_ratio >= 0.30:
        return "extreme_close_up"
    if face_ratio >= 0.10:
        return "single_close"
    if face_ratio >= 0.025:
        return "single_medium"
    return "wide_context"


def _confidence(face_ratios: list[float], gaze_scores: list[float]) -> float:
    if not face_ratios:
        return 0.48
    coverage = min(1.0, len(face_ratios) / 3.0)
    stability = 1.0
    if len(face_ratios) > 1:
        avg = mean(face_ratios)
        drift = mean(abs(value - avg) for value in face_ratios)
        stability = max(0.0, 1.0 - drift * 12.0)
    gaze_bonus = 0.08 if gaze_scores else 0.0
    return max(0.0, min(1.0, 0.55 + coverage * 0.22 + stability * 0.15 + gaze_bonus))


def _region_for_shot(
    shot: dict[str, Any],
    face_body: dict[str, Any] | None,
    gaze_body: dict[str, Any] | None,
) -> dict[str, Any]:
    start_s = float(shot.get("start_s", shot.get("start", 0.0)))
    end_s = float(shot.get("end_s", shot.get("end", start_s)))
    face_data = face_body or {}
    gaze_data = gaze_body or {}
    face_ratios = _face_ratios_in_window(
        face_data.get("per_frame", []),
        int(face_data.get("detect_width", 0)),
        int(face_data.get("detect_height", 0)),
        start_s,
        end_s,
    )
    gaze_scores = _gaze_in_window(gaze_data.get("per_frame", []), start_s, end_s)
    face_ratio = mean(face_ratios) if face_ratios else 0.0
    return {
        "start_s": start_s,
        "end_s": end_s,
        "composition_source": "heuristic:composition-v1",
        "composition_confidence": _confidence(face_ratios, gaze_scores),
        "subject_role": _subject_role(face_ratio, gaze_scores),
        "depth_layer": _depth_layer(face_ratio),
        "framing": _framing(face_ratio),
        "face_area_ratio": face_ratio,
        "samples": len(face_ratios),
    }


def _overlap_seconds(a_start: float, a_end: float, b_start: float, b_end: float) -> float:
    return max(0.0, min(a_end, b_end) - max(a_start, b_start))


def _model_region_for_shot(
    shot: dict[str, Any],
    model_body: dict[str, Any] | None,
) -> dict[str, Any] | None:
    regions = (model_body or {}).get("regions", [])
    start_s = float(shot.get("start_s", shot.get("start", 0.0)))
    end_s = float(shot.get("end_s", shot.get("end", start_s)))
    best: tuple[float, dict[str, Any]] | None = None
    for region in regions:
        source = str(region.get("composition_source", ""))
        if not source.startswith("model:"):
            continue
        overlap = _overlap_seconds(
            start_s,
            end_s,
            float(region.get("start_s", region.get("start", start_s))),
            float(region.get("end_s", region.get("end", end_s))),
        )
        if overlap <= 0.0:
            continue
        if best is None or overlap > best[0]:
            best = (overlap, region)
    return dict(best[1]) if best else None


def _merge_model_region(
    heuristic: dict[str, Any],
    model_region: dict[str, Any] | None,
) -> dict[str, Any]:
    if not model_region:
        return heuristic
    merged = dict(heuristic)
    for key in [
        "composition_source",
        "composition_confidence",
        "subject_role",
        "depth_layer",
        "framing",
    ]:
        if key in model_region:
            merged[f"heuristic_{key}"] = heuristic.get(key)
            merged[key] = model_region[key]
    return merged


def _regions_from_sidecars(
    scenes_body: dict[str, Any],
    face_body: dict[str, Any] | None,
    gaze_body: dict[str, Any] | None,
    model_body: dict[str, Any] | None = None,
) -> list[dict[str, Any]]:
    shots = scenes_body.get("shots", [])
    if not shots:
        duration_s = float(scenes_body.get("duration_s", 0.0))
        shots = [{"index": 0, "start_s": 0.0, "end_s": duration_s}]
    return [
        _merge_model_region(
            _region_for_shot(shot, face_body, gaze_body),
            _model_region_for_shot(shot, model_body),
        )
        for shot in shots
    ]


def _verify_regions(regions: list[dict[str, Any]]) -> dict[str, Any]:
    issues: list[str] = []
    previous_end: float | None = None
    for index, region in enumerate(regions):
        start_s = float(region.get("start_s", region.get("start", 0.0)))
        end_s = float(region.get("end_s", region.get("end", start_s)))
        if not (start_s < end_s and start_s >= 0.0):
            issues.append(f"region {index} has non-positive or non-finite range")
        if previous_end is not None and start_s < previous_end:
            issues.append(f"region {index} overlaps the previous region")
        previous_end = end_s
        confidence = region.get("composition_confidence")
        if confidence is None or not (0.0 <= float(confidence) <= 1.0):
            issues.append(f"region {index} composition_confidence is outside 0..=1")
        if not str(region.get("composition_source", "")).strip():
            issues.append(f"region {index} missing composition_source")
    return {
        "passed": not issues,
        "checked_regions": len(regions),
        "issues": issues,
    }


def _handle(req: IndexAssetRequest) -> dict[str, Any]:
    project_root = _project_root_from(req)
    scenes_doc = _read_sidecar(project_root, "scenedetect", req.asset_id)
    if not scenes_doc:
        raise RuntimeError(
            f"composition-mcp: missing scenedetect sidecar at "
            f"<project>/index/scenedetect/{req.asset_id}.json"
        )
    scenes_body = scenes_doc.get("data", {})
    face_doc = _read_sidecar(project_root, "face", req.asset_id)
    gaze_doc = _read_sidecar(project_root, "gaze", req.asset_id)
    model_doc = _read_sidecar(project_root, "composition-model", req.asset_id)
    regions = _regions_from_sidecars(
        scenes_body,
        face_doc.get("data", {}) if face_doc else None,
        gaze_doc.get("data", {}) if gaze_doc else None,
        model_doc.get("data", {}) if model_doc else None,
    )
    _log.info(
        "composition-mcp: asset=%s regions=%d (face_data=%s gaze_data=%s model_data=%s)",
        req.asset_id,
        len(regions),
        bool(face_doc),
        bool(gaze_doc),
        bool(model_doc),
    )
    return {
        "regions": regions,
        "verification": _verify_regions(regions),
        "depends_on": ["scenedetect", "face", "gaze", "composition-model"],
    }


handle = server.index_asset(_handle) if server else _handle


def main() -> None:
    if server is None:
        raise RuntimeError("composition-mcp requires awidat-mcp runtime dependencies")
    server.run()
