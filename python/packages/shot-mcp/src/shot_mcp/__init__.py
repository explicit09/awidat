"""Shot-type + camera-motion classifier for Awidat.

Reads `index/scenedetect/<asset>.json` for shot boundaries,
`index/face/<asset>.json` for face boxes,
`index/gaze/<asset>.json` for eye-contact signals,
`index/clip/<asset>.json` for CLIP visual similarity, and optional
`index/composition/<asset>.json` model-backed composition labels. All are
fail-soft except scenedetect.
For each shot, emits:

- `type`: extreme-close-up | close-up | medium | wide | no-face
  (derived from average face-area-to-frame-area ratio across the shot)
- `motion`: static | slow-pan | fast-cut | handheld
  (derived from sparse optical-flow magnitude sampled inside the shot)
- edit-aware fields for transition/cut grammar: `dominant_direction`,
  `subject_center`, `face_center`, `gaze_score`, `at_camera_ratio`,
  `eye_trace`, `subject_zone`, `face_zone`, `headroom`, `look_space`,
  `composition_source`, `composition_confidence`, `subject_role`,
  `depth_layer`, `framing`, `match_candidates`, `match_cut_score`,
  `action_score`, `whip_pan_score`, and `occlusion_score`

The shot list defines the time grid; this indexer adds labels and
edit-aware signals per shot. The agent uses these for editorial choices like
'find the cutaway b-rolls' (no-face + handheld) or 'pull the
intimate close-ups' (close-up + static).

Schema version: "1".
"""

from __future__ import annotations

import base64
import json
import logging
import subprocess
from pathlib import Path
from typing import Any

import cv2
import numpy as np

from awidat_mcp import IndexAssetRequest, IndexerServer

INDEXER_NAME = "shot"
INDEXER_VERSION = "0.1.0"
SCHEMA_VERSION = "1"

# Face-area / frame-area thresholds for shot type. Tuned on
# talking-head interview footage; adjust if you cut a lot of music
# performance.
SHOT_TYPE_THRESHOLDS = {
    "extreme-close-up": 0.30,  # face fills > 30 % of frame
    "close-up": 0.10,          # 10–30 %
    "medium": 0.025,           # 2.5–10 %
    # below 2.5 % → wide
}

# Optical-flow magnitude buckets (mean per-pixel pixel-distance).
MOTION_BUCKETS = [
    ("static", 0.0, 0.5),
    ("slow-pan", 0.5, 3.0),
    ("handheld", 3.0, 8.0),
    ("fast-cut", 8.0, float("inf")),
]

# Sample three flow probes per shot — start, middle, end. More than
# that is wasteful for the granularity we need.
FLOW_PROBES_PER_SHOT = 3

# Probe at this width — small enough that Farneback is fast, large
# enough that motion magnitude is meaningful.
PROBE_WIDTH = 320

# CLIP frame embeddings are sampled densely enough that adjacent frames
# often describe the same visual opportunity. Keep only one candidate per
# short temporal neighborhood before applying the result limit.
MATCH_CANDIDATE_DEDUP_WINDOW_S = 0.75

# Built-in motion sidecars are normalized to roughly [0, 1]. Prefer them over
# per-shot ffmpeg seeks; fall back to optical flow only when the sidecar is absent.
MOTION_SIGNAL_BUCKETS = [
    ("static", 0.0, 0.08),
    ("slow-pan", 0.08, 0.35),
    ("handheld", 0.35, 0.75),
    ("fast-cut", 0.75, float("inf")),
]

_log = logging.getLogger(INDEXER_NAME)


server = IndexerServer(
    name=INDEXER_NAME,
    indexer_version=INDEXER_VERSION,
    schema_version=SCHEMA_VERSION,
)


def _project_root_from(req: IndexAssetRequest) -> Path:
    """Walk up from the asset to the dir containing `index/`."""
    if req.project_root:
        return Path(req.project_root).absolute()
    p = Path(req.asset_path).absolute()
    while p != p.parent:
        if (p / "index").is_dir():
            return p
        p = p.parent
    return Path(req.asset_path).absolute().parent


def _index_root_from(req: IndexAssetRequest, project_root: Path) -> Path:
    if req.index_root:
        return Path(req.index_root).absolute()
    return project_root / "index"


def _read_sidecar(index_root: Path, indexer: str, asset_id: str) -> dict[str, Any] | None:
    p = index_root / indexer / f"{asset_id}.json"
    if not p.exists():
        return None
    try:
        return json.loads(p.read_text())
    except (OSError, json.JSONDecodeError):
        return None


def _read_clip_bodies(index_root: Path) -> list[tuple[str, dict[str, Any]]]:
    clip_dir = index_root / "clip"
    if not clip_dir.exists():
        return []
    bodies: list[tuple[str, dict[str, Any]]] = []
    for path in clip_dir.rglob("*.json"):
        try:
            doc = json.loads(path.read_text())
        except (OSError, json.JSONDecodeError):
            continue
        asset_id = path.relative_to(clip_dir).as_posix()
        if asset_id.endswith(".json"):
            asset_id = asset_id[:-5]
        data = doc.get("data", {})
        if isinstance(data, dict):
            bodies.append((asset_id, data))
    return bodies


def _classify_shot_type(face_ratio: float) -> str:
    if face_ratio == 0.0:
        return "no-face"
    if face_ratio >= SHOT_TYPE_THRESHOLDS["extreme-close-up"]:
        return "extreme-close-up"
    if face_ratio >= SHOT_TYPE_THRESHOLDS["close-up"]:
        return "close-up"
    if face_ratio >= SHOT_TYPE_THRESHOLDS["medium"]:
        return "medium"
    return "wide"


def _bucket_motion(magnitude: float) -> str:
    for label, lo, hi in MOTION_BUCKETS:
        if lo <= magnitude < hi:
            return label
    return "static"


def _bucket_motion_signal(magnitude: float) -> str:
    for label, lo, hi in MOTION_SIGNAL_BUCKETS:
        if lo <= magnitude < hi:
            return label
    return "static"


def _read_motion_signal(project_root: Path, asset_path: str) -> dict[str, Any] | None:
    motion_dir = project_root / ".awidat" / "motion"
    if not motion_dir.exists():
        return None
    stem = Path(asset_path).stem
    candidates = list(motion_dir.glob(f"{stem}-*.json"))
    if not candidates:
        return None
    newest = max(candidates, key=lambda p: p.stat().st_mtime)
    try:
        body = json.loads(newest.read_text())
    except (OSError, json.JSONDecodeError):
        return None
    magnitudes = body.get("magnitudes")
    samples_per_second = body.get("samples_per_second", 1)
    if not isinstance(magnitudes, list) or not magnitudes:
        return None
    try:
        sps = float(samples_per_second)
    except (TypeError, ValueError):
        sps = 1.0
    return {
        "samples_per_second": max(sps, 0.001),
        "magnitudes": [float(value) for value in magnitudes],
    }


def _motion_summary_from_signal(
    motion_signal: dict[str, Any] | None,
    start_s: float,
    end_s: float,
) -> dict[str, Any] | None:
    if not motion_signal:
        return None
    sps = float(motion_signal["samples_per_second"])
    magnitudes = motion_signal["magnitudes"]
    start_i = max(0, int(start_s * sps))
    end_i = min(len(magnitudes), max(start_i + 1, int(np.ceil(end_s * sps))))
    window = magnitudes[start_i:end_i]
    if not window:
        return None
    mag = float(np.mean(window))
    return {
        "motion": _bucket_motion_signal(mag),
        "motion_magnitude": mag,
        "dominant_direction": None,
        "action_score": _score_between(mag, 0.08, 0.75),
        "whip_pan_score": 0.0,
        "occlusion_score": 0.0,
    }


def _grab_frame(asset_path: str, t_s: float, target_w: int) -> np.ndarray | None:
    """Pull a single frame at `t_s` via ffmpeg seek-then-extract.
    Returns HxWx3 uint8 BGR (cv2 convention) or None on failure."""
    cmd = [
        "ffmpeg",
        "-v",
        "error",
        "-ss",
        f"{t_s}",
        "-i",
        asset_path,
        "-frames:v",
        "1",
        "-vf",
        f"scale={target_w}:-2",
        "-pix_fmt",
        "bgr24",
        "-f",
        "rawvideo",
        "-",
    ]
    try:
        proc = subprocess.run(cmd, check=True, capture_output=True)
    except subprocess.CalledProcessError:
        return None
    raw = proc.stdout
    if not raw:
        return None
    # We don't know the height up front; ffmpeg picked it from -2.
    # Probe by size: bytes / (target_w * 3) = height.
    n_per_row = target_w * 3
    if len(raw) % n_per_row != 0:
        return None
    h = len(raw) // n_per_row
    return np.frombuffer(raw, dtype=np.uint8).reshape(h, target_w, 3).copy()


def _flow_magnitude(prev: np.ndarray, curr: np.ndarray) -> float:
    """Mean per-pixel pixel-distance via Farneback dense flow."""
    return _flow_direction_summary(prev, curr)["motion_magnitude"]


def _flow_direction_summary(prev: np.ndarray, curr: np.ndarray) -> dict[str, Any]:
    """Compact edit-aware optical-flow summary for one frame pair."""
    g_prev = cv2.cvtColor(prev, cv2.COLOR_BGR2GRAY)
    g_curr = cv2.cvtColor(curr, cv2.COLOR_BGR2GRAY)
    flow = cv2.calcOpticalFlowFarneback(
        g_prev,
        g_curr,
        None,
        pyr_scale=0.5,
        levels=3,
        winsize=15,
        iterations=2,
        poly_n=5,
        poly_sigma=1.2,
        flags=0,
    )
    mag = np.linalg.norm(flow, axis=2)
    motion_magnitude = float(mag.mean())
    mean_dx = float(flow[:, :, 0].mean())
    mean_dy = float(flow[:, :, 1].mean())
    gray_delta = np.abs(g_curr.astype(np.int16) - g_prev.astype(np.int16))
    signed_delta = g_curr.astype(np.int16) - g_prev.astype(np.int16)
    dark_fraction = float((g_curr < 24).mean())
    changed_fraction = float((gray_delta > 48).mean())
    dark_occlusion = dark_fraction if dark_fraction > 0.9 else 0.0
    occlusion_score = max(dark_occlusion, changed_fraction * 0.8)
    dominant_direction = _dominant_direction(mean_dx, mean_dy, motion_magnitude)
    if dominant_direction is None and changed_fraction > 0.05:
        dominant_direction = _difference_direction(signed_delta)

    return {
        "motion_magnitude": motion_magnitude,
        "dominant_direction": dominant_direction,
        "action_score": max(
            _score_between(motion_magnitude, 0.5, 8.0),
            _clamp01(changed_fraction * 1.5),
        ),
        "whip_pan_score": _score_between(abs(mean_dx), 2.5, 12.0)
        if dominant_direction in {"left", "right"}
        else 0.0,
        "occlusion_score": _clamp01(occlusion_score),
    }


def _dominant_direction(
    mean_dx: float, mean_dy: float, motion_magnitude: float
) -> str | None:
    if motion_magnitude < 0.5:
        return None
    if abs(mean_dx) >= abs(mean_dy) * 1.25:
        return "right" if mean_dx > 0.0 else "left"
    if abs(mean_dy) >= abs(mean_dx) * 1.25:
        return "down" if mean_dy > 0.0 else "up"
    return None


def _difference_direction(signed_delta: np.ndarray) -> str | None:
    positive = signed_delta > 32
    negative = signed_delta < -32
    if positive.sum() == 0 or negative.sum() == 0:
        return None
    pos_y, pos_x = np.argwhere(positive).mean(axis=0)
    neg_y, neg_x = np.argwhere(negative).mean(axis=0)
    dx = float(pos_x - neg_x)
    dy = float(pos_y - neg_y)
    if abs(dx) >= abs(dy) * 1.25:
        return "right" if dx > 0.0 else "left"
    if abs(dy) >= abs(dx) * 1.25:
        return "down" if dy > 0.0 else "up"
    return None


def _score_between(value: float, lo: float, hi: float) -> float:
    if hi <= lo:
        return 0.0
    return _clamp01((value - lo) / (hi - lo))


def _clamp01(value: float) -> float:
    return max(0.0, min(1.0, float(value)))


def _avg_face_ratio_in_window(
    per_frame: list[dict[str, Any]],
    detect_w: int,
    detect_h: int,
    start_s: float,
    end_s: float,
) -> float:
    """Average (max-face-area / frame-area) for face sidecar entries
    whose t_s falls inside [start_s, end_s)."""
    total = detect_w * detect_h
    if total == 0:
        return 0.0
    ratios: list[float] = []
    for entry in per_frame:
        t = entry["t_s"]
        if not (start_s <= t < end_s):
            continue
        if not entry["faces"]:
            ratios.append(0.0)
            continue
        max_area = 0
        for f in entry["faces"]:
            top, right, bottom, left = f["box"]
            max_area = max(max_area, max(0, bottom - top) * max(0, right - left))
        ratios.append(max_area / total)
    if not ratios:
        return 0.0
    return float(np.mean(ratios))


def _face_center_in_window(
    per_frame: list[dict[str, Any]],
    detect_w: int,
    detect_h: int,
    start_s: float,
    end_s: float,
) -> list[float] | None:
    """Area-weighted center of the largest detected face in each frame.

    Coordinates are normalized to `[0, 1]` frame space so downstream tools can
    compare screen position without knowing the detector resolution.
    """
    if detect_w <= 0 or detect_h <= 0:
        return None
    weighted_x = 0.0
    weighted_y = 0.0
    total_area = 0.0
    for entry in per_frame:
        t = entry["t_s"]
        if not (start_s <= t < end_s):
            continue
        largest_face: dict[str, Any] | None = None
        largest_area = 0.0
        for face in entry["faces"]:
            top, right, bottom, left = face["box"]
            area = float(max(0, bottom - top) * max(0, right - left))
            if area > largest_area:
                largest_area = area
                largest_face = face
        if largest_face is None or largest_area == 0.0:
            continue
        top, right, bottom, left = largest_face["box"]
        center_x = float(left + right) / 2.0 / float(detect_w)
        center_y = float(top + bottom) / 2.0 / float(detect_h)
        weighted_x += center_x * largest_area
        weighted_y += center_y * largest_area
        total_area += largest_area
    if total_area == 0.0:
        return None
    return [_clamp01(weighted_x / total_area), _clamp01(weighted_y / total_area)]


def _gaze_summary_in_window(
    per_frame: list[dict[str, Any]],
    start_s: float,
    end_s: float,
) -> dict[str, Any] | None:
    """Aggregate gaze sidecar frames into one shot-level eye-trace summary."""
    scores: list[float] = []
    at_camera: list[bool] = []
    for entry in per_frame:
        t = entry["t_s"]
        if not (start_s <= t < end_s):
            continue
        largest_face: dict[str, Any] | None = None
        largest_area = 0.0
        for face in entry["faces"]:
            if "gaze_score" not in face:
                continue
            top, right, bottom, left = face.get("box", [0, 0, 0, 0])
            area = float(max(0, bottom - top) * max(0, right - left))
            if area > largest_area:
                largest_area = area
                largest_face = face
        if largest_face is None:
            continue
        scores.append(float(largest_face["gaze_score"]))
        at_camera.append(bool(largest_face.get("at_camera", False)))
    if not scores:
        return None
    at_camera_ratio = float(np.mean(at_camera)) if at_camera else 0.0
    gaze_score = float(np.mean(scores))
    return {
        "gaze_score": gaze_score,
        "at_camera_ratio": at_camera_ratio,
        "eye_trace": _eye_trace_bucket(gaze_score, at_camera_ratio),
        "samples": len(scores),
    }


def _eye_trace_bucket(gaze_score: float, at_camera_ratio: float) -> str:
    if at_camera_ratio >= 0.67:
        return "at_camera"
    if at_camera_ratio <= 0.33:
        if gaze_score <= -0.15:
            return "left"
        if gaze_score >= 0.15:
            return "right"
        return "off_camera"
    return "mixed"


def _composition_summary(
    subject_center: list[float] | None,
    face_center: list[float] | None,
    gaze_summary: dict[str, Any] | None,
) -> dict[str, str | None]:
    """Derive editor-facing composition semantics from normalized centers.

    These labels are deliberately coarse. They give downstream tools stable
    language for eye-trace/look-space continuity without pretending the face
    detector knows full cinematography intent.
    """
    anchor = subject_center or face_center
    eye_trace = gaze_summary.get("eye_trace") if gaze_summary else None
    return {
        "subject_zone": _horizontal_zone(anchor[0]) if anchor else None,
        "face_zone": _frame_zone(face_center) if face_center else None,
        "headroom": _headroom(face_center[1]) if face_center else None,
        "look_space": _look_space(anchor, eye_trace) if anchor else None,
    }


MODEL_COMPOSITION_KEYS = (
    "composition_source",
    "composition_confidence",
    "subject_role",
    "depth_layer",
    "framing",
)


def _composition_labels_for_shot(
    composition_body: dict[str, Any] | None,
    start_s: float,
    end_s: float,
) -> dict[str, Any]:
    """Pick model-backed composition labels overlapping this shot window."""
    empty = {key: None for key in MODEL_COMPOSITION_KEYS}
    if not isinstance(composition_body, dict):
        return empty
    regions = composition_body.get("regions", [])
    if not isinstance(regions, list):
        return empty
    best_region: dict[str, Any] | None = None
    best_score = 0.0
    for region in regions:
        if not isinstance(region, dict):
            continue
        region_start = float(region.get("start_s", region.get("start", 0.0)))
        region_end = float(region.get("end_s", region.get("end", region_start)))
        overlap = max(0.0, min(end_s, region_end) - max(start_s, region_start))
        if overlap <= 0.0:
            continue
        confidence = float(region.get("composition_confidence", 0.0) or 0.0)
        score = overlap + confidence * 0.001
        if score > best_score:
            best_score = score
            best_region = region
    if best_region is None:
        return empty
    return {key: best_region.get(key) for key in MODEL_COMPOSITION_KEYS}


def _horizontal_zone(x: float) -> str:
    if x < 1.0 / 3.0:
        return "left_third"
    if x > 2.0 / 3.0:
        return "right_third"
    return "center"


def _vertical_zone(y: float) -> str:
    if y < 1.0 / 3.0:
        return "upper"
    if y > 2.0 / 3.0:
        return "lower"
    return "middle"


def _frame_zone(center: list[float]) -> str:
    horizontal = _horizontal_zone(center[0]).replace("_third", "")
    vertical = _vertical_zone(center[1])
    if horizontal == "center" and vertical == "middle":
        return "center"
    return f"{vertical}_{horizontal}"


def _headroom(face_y: float) -> str:
    if face_y < 0.22:
        return "tight"
    if face_y > 0.46:
        return "loose"
    return "balanced"


def _look_space(center: list[float], eye_trace: Any) -> str | None:
    if eye_trace == "at_camera":
        return "direct"
    if eye_trace == "mixed":
        return "mixed"
    if eye_trace not in {"left", "right"}:
        return None
    x = center[0]
    if eye_trace == "right":
        return "open" if x <= 0.5 else "closed"
    return "open" if x >= 0.5 else "closed"


def _decode_clip_embeddings(body: dict[str, Any]) -> np.ndarray | None:
    frame_count = int(body.get("frame_count", 0))
    dim = int(body.get("embedding_dim", 0))
    encoded = body.get("embeddings_b64")
    if frame_count <= 0 or dim <= 0 or not isinstance(encoded, str):
        return None
    try:
        raw = base64.b64decode(encoded)
    except (TypeError, ValueError):
        return None
    expected = frame_count * dim * 2
    if len(raw) != expected:
        return None
    matrix = np.frombuffer(raw, dtype=np.float16).reshape(frame_count, dim).astype(np.float32)
    norms = np.linalg.norm(matrix, axis=1, keepdims=True)
    norms[norms == 0.0] = 1.0
    return matrix / norms


def _prepare_clip_index(
    clip_body: dict[str, Any] | None,
    candidate_clip_bodies: list[tuple[str | None, dict[str, Any]]],
) -> tuple[np.ndarray, np.ndarray, list[dict[str, Any]]] | None:
    if not clip_body:
        return None
    query_embeddings = _decode_clip_embeddings(clip_body)
    if query_embeddings is None:
        return None
    query_timestamps = clip_body.get("timestamps_s", [])
    if (
        not isinstance(query_timestamps, list)
        or len(query_timestamps) != query_embeddings.shape[0]
    ):
        query_timestamps = [float(i) for i in range(query_embeddings.shape[0])]

    candidates: list[dict[str, Any]] = []
    for candidate_asset_id, candidate_body in candidate_clip_bodies:
        candidate_embeddings = _decode_clip_embeddings(candidate_body)
        if (
            candidate_embeddings is None
            or candidate_embeddings.shape[1] != query_embeddings.shape[1]
        ):
            continue
        candidate_timestamps = candidate_body.get("timestamps_s", [])
        if (
            not isinstance(candidate_timestamps, list)
            or len(candidate_timestamps) != candidate_embeddings.shape[0]
        ):
            candidate_timestamps = [float(i) for i in range(candidate_embeddings.shape[0])]
        candidates.append(
            {
                "asset_id": candidate_asset_id,
                "embeddings": candidate_embeddings,
                "timestamps_s": np.asarray(
                    [float(t) for t in candidate_timestamps],
                    dtype=np.float32,
                ),
            }
        )
    return (
        query_embeddings,
        np.asarray([float(t) for t in query_timestamps], dtype=np.float32),
        candidates,
    )


def _clip_match_candidates_for_shot(
    clip_index: tuple[np.ndarray, np.ndarray, list[dict[str, Any]]] | None,
    start_s: float,
    end_s: float,
    limit: int = 3,
    asset_id: str | None = None,
) -> list[dict[str, Any]]:
    if not clip_index:
        return []
    embeddings, timestamps, candidates = clip_index
    if embeddings.size == 0 or timestamps.size == 0 or not candidates:
        return []
    center_s = (start_s + end_s) / 2.0
    insertion = int(np.searchsorted(timestamps, center_s))
    query_index = min(max(insertion, 0), len(timestamps) - 1)
    if query_index > 0 and abs(float(timestamps[query_index - 1]) - center_s) < abs(
        float(timestamps[query_index]) - center_s
    ):
        query_index -= 1
    query = embeddings[query_index]
    scored: list[dict[str, Any]] = []
    for candidate in candidates:
        candidate_asset_id = candidate["asset_id"]
        candidate_embeddings = candidate["embeddings"]
        candidate_timestamps = candidate["timestamps_s"]
        similarities = candidate_embeddings @ query
        if candidate_asset_id == asset_id:
            in_current_shot = (candidate_timestamps >= start_s) & (candidate_timestamps < end_s)
            if bool(in_current_shot.all()):
                continue
            similarities = similarities.copy()
            similarities[in_current_shot] = -np.inf
        finite = np.isfinite(similarities)
        if not bool(finite.any()):
            continue
        top_count = min(max(limit * 16, 64), int(finite.sum()))
        finite_indices = np.flatnonzero(finite)
        finite_scores = similarities[finite_indices]
        if finite_scores.size > top_count:
            local_top = np.argpartition(finite_scores, -top_count)[-top_count:]
            top_indices = finite_indices[local_top]
        else:
            top_indices = finite_indices
        top_indices = top_indices[np.argsort(similarities[top_indices])[::-1]]
        for idx in top_indices:
            t_s = float(candidate_timestamps[idx])
            scored.append(
                {
                    "time_s": t_s,
                    "similarity": float(similarities[idx]),
                    "basis": "clip_embedding",
                }
            )
            if candidate_asset_id is not None:
                scored[-1]["asset_id"] = candidate_asset_id
    scored.sort(key=lambda item: item["similarity"], reverse=True)
    diversified = _deduplicate_match_candidates(scored)
    for idx, item in enumerate(diversified):
        next_similarity = (
            diversified[idx + 1]["similarity"] if idx + 1 < len(diversified) else 0.0
        )
        margin = max(0.0, float(item["similarity"]) - float(next_similarity))
        item["confidence"] = _clamp01(float(item["similarity"]) * 0.75 + margin * 1.5)
    return diversified[:limit]


def _deduplicate_match_candidates(candidates: list[dict[str, Any]]) -> list[dict[str, Any]]:
    diversified: list[dict[str, Any]] = []
    for candidate in candidates:
        candidate_asset = candidate.get("asset_id")
        candidate_time = float(candidate["time_s"])
        if any(
            existing.get("asset_id") == candidate_asset
            and abs(float(existing["time_s"]) - candidate_time)
            <= MATCH_CANDIDATE_DEDUP_WINDOW_S
            for existing in diversified
        ):
            continue
        diversified.append(candidate)
    return diversified


@server.index_asset
def handle(req: IndexAssetRequest) -> dict[str, Any]:
    project_root = _project_root_from(req)
    index_root = _index_root_from(req, project_root)

    scenes_doc = _read_sidecar(index_root, "scenedetect", req.asset_id)
    if not scenes_doc:
        raise RuntimeError(
            f"shot-mcp: missing scenedetect sidecar at "
            f"<project>/index/scenedetect/{req.asset_id}.json — "
            "run scenedetect-mcp first."
        )
    shots = scenes_doc.get("data", {}).get("shots", [])
    if not shots:
        # Treat the whole asset as one shot — better than nothing.
        duration_s = float(scenes_doc.get("data", {}).get("duration_s", 0.0))
        shots = [
            {"index": 0, "start_s": 0.0, "end_s": duration_s}
        ]

    face_doc = _read_sidecar(index_root, "face", req.asset_id)
    per_frame_faces = (
        face_doc.get("data", {}).get("per_frame", []) if face_doc else []
    )
    detect_w = int(face_doc.get("data", {}).get("detect_width", 0)) if face_doc else 0
    detect_h = int(face_doc.get("data", {}).get("detect_height", 0)) if face_doc else 0
    gaze_doc = _read_sidecar(index_root, "gaze", req.asset_id)
    per_frame_gaze = (
        gaze_doc.get("data", {}).get("per_frame", []) if gaze_doc else []
    )
    clip_doc = _read_sidecar(index_root, "clip", req.asset_id)
    clip_body = clip_doc.get("data", {}) if clip_doc else None
    composition_doc = _read_sidecar(index_root, "composition", req.asset_id)
    composition_body = composition_doc.get("data", {}) if composition_doc else None
    candidate_clip_bodies = _read_clip_bodies(index_root)
    if not candidate_clip_bodies and clip_body:
        candidate_clip_bodies = [(req.asset_id, clip_body)]
    clip_index = _prepare_clip_index(clip_body, candidate_clip_bodies)
    motion_signal = _read_motion_signal(project_root, req.asset_path)

    out_shots: list[dict[str, Any]] = []
    for shot in shots:
        start_s = float(shot["start_s"])
        end_s = float(shot["end_s"])
        face_ratio = _avg_face_ratio_in_window(
            per_frame_faces, detect_w, detect_h, start_s, end_s
        )
        face_center = _face_center_in_window(
            per_frame_faces, detect_w, detect_h, start_s, end_s
        )
        gaze_summary = _gaze_summary_in_window(per_frame_gaze, start_s, end_s)
        composition = _composition_summary(face_center, face_center, gaze_summary)
        model_composition = _composition_labels_for_shot(
            composition_body, start_s, end_s
        )
        match_candidates = _clip_match_candidates_for_shot(
            clip_index,
            start_s,
            end_s,
            asset_id=req.asset_id,
        )
        match_cut_score = (
            match_candidates[0]["similarity"] if match_candidates else None
        )
        shot_type = _classify_shot_type(face_ratio)
        motion_summary = _motion_summary_from_signal(motion_signal, start_s, end_s)
        if motion_summary is None:
            # Fallback only: sample optical flow at start, middle, end; pair adjacent samples.
            probes = max(2, FLOW_PROBES_PER_SHOT)
            summaries: list[dict[str, Any]] = []
            if end_s - start_s < 0.2:
                mag = 0.0
            else:
                ts = np.linspace(start_s + 0.05, end_s - 0.05, probes)
                mags: list[float] = []
                prev_frame: np.ndarray | None = None
                for t in ts:
                    f = _grab_frame(req.asset_path, float(t), PROBE_WIDTH)
                    if f is None:
                        continue
                    if prev_frame is not None and prev_frame.shape == f.shape:
                        summary = _flow_direction_summary(prev_frame, f)
                        summaries.append(summary)
                        mags.append(float(summary["motion_magnitude"]))
                    prev_frame = f
                mag = float(np.mean(mags)) if mags else 0.0
            action_score = _score_between(mag, 0.5, 8.0)
            whip_pan_score = 0.0
            occlusion_score = 0.0
            dominant_direction = None
            if summaries:
                action_score = float(np.mean([s["action_score"] for s in summaries]))
                whip_pan_score = float(np.mean([s["whip_pan_score"] for s in summaries]))
                occlusion_score = float(np.max([s["occlusion_score"] for s in summaries]))
                directions = [
                    s["dominant_direction"]
                    for s in summaries
                    if s["dominant_direction"] is not None
                ]
                if directions:
                    dominant_direction = max(set(directions), key=directions.count)
            motion_summary = {
                "motion": _bucket_motion(mag),
                "motion_magnitude": mag,
                "dominant_direction": dominant_direction,
                "action_score": action_score,
                "whip_pan_score": whip_pan_score,
                "occlusion_score": occlusion_score,
            }

        out_shots.append(
            {
                "index": int(shot.get("index", len(out_shots))),
                "start_s": start_s,
                "end_s": end_s,
                "type": shot_type,
                "face_area_ratio": face_ratio,
                "subject_center": face_center,
                "face_center": face_center,
                "subject_zone": composition["subject_zone"],
                "face_zone": composition["face_zone"],
                "headroom": composition["headroom"],
                "look_space": composition["look_space"],
                "composition_source": model_composition["composition_source"],
                "composition_confidence": model_composition["composition_confidence"],
                "subject_role": model_composition["subject_role"],
                "depth_layer": model_composition["depth_layer"],
                "framing": model_composition["framing"],
                "gaze_score": gaze_summary["gaze_score"] if gaze_summary else None,
                "at_camera_ratio": gaze_summary["at_camera_ratio"] if gaze_summary else None,
                "eye_trace": gaze_summary["eye_trace"] if gaze_summary else None,
                "gaze_samples": gaze_summary["samples"] if gaze_summary else 0,
                "match_candidates": match_candidates,
                "match_cut_score": match_cut_score,
                "motion": motion_summary["motion"],
                "motion_magnitude": motion_summary["motion_magnitude"],
                "dominant_direction": motion_summary["dominant_direction"],
                "action_score": motion_summary["action_score"],
                "whip_pan_score": motion_summary["whip_pan_score"],
                "occlusion_score": motion_summary["occlusion_score"],
            }
        )

    _log.info(
        "shot-mcp: asset=%s shots=%d (face_data=%s gaze_data=%s clip_data=%s composition_data=%s motion_signal=%s)",
        req.asset_id,
        len(out_shots),
        bool(face_doc),
        bool(gaze_doc),
        bool(clip_doc),
        bool(composition_doc),
        bool(motion_signal),
    )

    return {
        "shots": out_shots,
        "thresholds": {
            "shot_type": SHOT_TYPE_THRESHOLDS,
            "motion_buckets": [
                {"label": lbl, "lo": lo, "hi": hi if hi != float("inf") else None}
                for lbl, lo, hi in MOTION_BUCKETS
            ],
        },
        "depends_on": ["scenedetect", "face", "gaze", "clip", "composition"],
    }


def main() -> None:
    server.run()
