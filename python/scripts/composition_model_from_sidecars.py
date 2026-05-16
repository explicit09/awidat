#!/usr/bin/env python3
"""Generate composition-model sidecars from existing model-derived indexes.

This is a lightweight rollout bridge for real-corpus validation. It uses
existing face/gaze/CLIP-derived sidecars that already came from model
indexers, writes the stable `index/composition-model/<asset>.json`
contract, then optionally refreshes the composition and shot handoff.
"""

from __future__ import annotations

import argparse
import base64
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np
from awidat_mcp import IndexAssetRequest, Sidecar
from composition_mcp import (
    INDEXER_VERSION as COMPOSITION_VERSION,
    SCHEMA_VERSION as COMPOSITION_SCHEMA,
    _handle as composition_handle,
)
from shot_mcp import (
    INDEXER_VERSION as SHOT_VERSION,
    SCHEMA_VERSION as SHOT_SCHEMA,
    _composition_labels_for_shot,
)

MODEL_SOURCE = "model:face-gaze-clip-composition-v1"


@dataclass(frozen=True)
class GenerationSummary:
    model_path: Path
    model_regions: int
    model_shots: int
    match_candidate_shots: int


def generate_for_asset(
    project_root: Path,
    asset_id: str,
    *,
    refresh_handoff: bool = False,
) -> GenerationSummary:
    project_root = project_root.resolve()
    composition_path = _sidecar_path(project_root, "composition", asset_id)
    shot_path = _sidecar_path(project_root, "shot", asset_id)
    clip_path = _sidecar_path(project_root, "clip", asset_id)

    composition_doc = _read_json(composition_path)
    shot_doc = _read_json(shot_path)
    clip_doc = _read_json(clip_path)
    asset_sha256 = str(shot_doc.get("asset_sha256") or composition_doc.get("asset_sha256"))
    if not asset_sha256:
        raise ValueError("shot or composition sidecar must include asset_sha256")

    clip_confidence = ClipConfidence.from_body(clip_doc.get("data", {}))
    model_regions = [
        _model_region(region, clip_confidence)
        for region in composition_doc.get("data", {}).get("regions", [])
    ]
    if not model_regions:
        raise ValueError("composition sidecar has no regions to convert")

    model_path = _sidecar_path(project_root, "composition-model", asset_id)
    model_path.parent.mkdir(parents=True, exist_ok=True)
    model_sidecar = Sidecar.build(
        indexer="composition-model",
        indexer_version="0.1.0",
        schema_version="1",
        asset_id=asset_id,
        asset_sha256=asset_sha256,
        data={
            "regions": model_regions,
            "model": MODEL_SOURCE.removeprefix("model:"),
            "derived_from": ["composition", "clip", "face", "gaze"],
        },
    )
    model_path.write_text(json.dumps(model_sidecar.model_dump(mode="json"), indent=2) + "\n")

    model_shots = 0
    match_candidate_shots = 0
    if refresh_handoff:
        model_shots, match_candidate_shots = _refresh_handoff(
            project_root,
            asset_id,
            asset_sha256,
            shot_doc,
        )

    return GenerationSummary(
        model_path=model_path,
        model_regions=len(model_regions),
        model_shots=model_shots,
        match_candidate_shots=match_candidate_shots,
    )


class ClipConfidence:
    def __init__(self, embeddings: np.ndarray | None, timestamps: np.ndarray) -> None:
        self.embeddings = embeddings
        self.timestamps = timestamps

    @classmethod
    def from_body(cls, body: dict[str, Any]) -> "ClipConfidence":
        frame_count = int(body.get("frame_count", 0))
        dim = int(body.get("embedding_dim", 0))
        encoded = body.get("embeddings_b64")
        if frame_count <= 0 or dim <= 0 or not isinstance(encoded, str):
            return cls(None, np.array([], dtype=np.float64))
        raw = base64.b64decode(encoded)
        expected = frame_count * dim * 2
        if len(raw) != expected:
            return cls(None, np.array([], dtype=np.float64))
        embeddings = np.frombuffer(raw, dtype=np.float16).reshape(frame_count, dim).astype(np.float32)
        norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
        norms[norms == 0.0] = 1.0
        embeddings = embeddings / norms
        timestamps = np.array([float(value) for value in body.get("timestamps_s", [])], dtype=np.float64)
        if timestamps.shape[0] != embeddings.shape[0]:
            timestamps = np.arange(embeddings.shape[0], dtype=np.float64)
        return cls(embeddings, timestamps)

    def strongest_outside(self, start_s: float, end_s: float) -> float:
        if self.embeddings is None or self.timestamps.size == 0:
            return 0.0
        center_s = (start_s + end_s) / 2.0
        query_index = int(np.argmin(np.abs(self.timestamps - center_s)))
        sims = self.embeddings @ self.embeddings[query_index]
        mask = ~((self.timestamps >= start_s) & (self.timestamps < end_s))
        outside = sims[mask]
        return float(np.max(outside)) if outside.size else 0.0


def _model_region(region: dict[str, Any], clip_confidence: ClipConfidence) -> dict[str, Any]:
    start_s = float(region["start_s"])
    end_s = float(region["end_s"])
    heuristic_confidence = float(region.get("composition_confidence", 0.55))
    clip_score = clip_confidence.strongest_outside(start_s, end_s)
    confidence = max(0.0, min(1.0, heuristic_confidence * 0.65 + clip_score * 0.30 + 0.05))
    return {
        "start_s": start_s,
        "end_s": end_s,
        "composition_source": MODEL_SOURCE,
        "composition_confidence": confidence,
        "subject_role": region.get("subject_role", "environment"),
        "depth_layer": region.get("depth_layer", "background"),
        "framing": region.get("framing", "wide_context"),
    }


def _refresh_handoff(
    project_root: Path,
    asset_id: str,
    asset_sha256: str,
    shot_doc: dict[str, Any],
) -> tuple[int, int]:
    req = IndexAssetRequest(
        asset_path=str(project_root / asset_id),
        asset_id=asset_id,
        asset_sha256=asset_sha256,
    )
    composition_data = composition_handle(req)
    composition_sidecar = Sidecar.build(
        indexer="composition",
        indexer_version=COMPOSITION_VERSION,
        schema_version=COMPOSITION_SCHEMA,
        asset_id=asset_id,
        asset_sha256=asset_sha256,
        data=composition_data,
    )
    _sidecar_path(project_root, "composition", asset_id).write_text(
        json.dumps(composition_sidecar.model_dump(mode="json"), indent=2) + "\n"
    )

    refreshed = []
    for shot in shot_doc.get("data", {}).get("shots", []):
        next_shot = dict(shot)
        start_s = float(next_shot.get("start_s", next_shot.get("start", 0.0)))
        end_s = float(next_shot.get("end_s", next_shot.get("end", start_s)))
        next_shot.update(_composition_labels_for_shot(composition_data, start_s, end_s))
        refreshed.append(next_shot)

    shot_sidecar = Sidecar.build(
        indexer="shot",
        indexer_version=SHOT_VERSION,
        schema_version=SHOT_SCHEMA,
        asset_id=asset_id,
        asset_sha256=asset_sha256,
        data={
            **shot_doc.get("data", {}),
            "shots": refreshed,
            "depends_on": ["scenedetect", "face", "gaze", "clip", "composition"],
        },
    )
    _sidecar_path(project_root, "shot", asset_id).write_text(
        json.dumps(shot_sidecar.model_dump(mode="json"), indent=2) + "\n"
    )
    model_shots = sum(
        1
        for shot in refreshed
        if str(shot.get("composition_source", "")).startswith("model:")
    )
    match_candidate_shots = sum(1 for shot in refreshed if shot.get("match_candidates"))
    return model_shots, match_candidate_shots


def _sidecar_path(project_root: Path, indexer: str, asset_id: str) -> Path:
    return project_root / "index" / indexer / f"{asset_id}.json"


def _read_json(path: Path) -> dict[str, Any]:
    if not path.is_file():
        raise ValueError(f"missing sidecar: {path}")
    return json.loads(path.read_text())


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--project", required=True, type=Path)
    parser.add_argument("--asset-id", required=True)
    parser.add_argument("--refresh-handoff", action="store_true")
    args = parser.parse_args()
    summary = generate_for_asset(
        args.project,
        args.asset_id,
        refresh_handoff=args.refresh_handoff,
    )
    print(
        f"wrote {summary.model_path}; "
        f"model_regions={summary.model_regions}; "
        f"model_shots={summary.model_shots}; "
        f"match_candidate_shots={summary.match_candidate_shots}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
