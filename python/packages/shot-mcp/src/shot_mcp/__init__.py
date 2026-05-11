"""Shot-type + camera-motion classifier for Awidat.

Reads `index/scenedetect/<asset>.json` for shot boundaries and
`index/face/<asset>.json` for face boxes (both fail-soft). For each
shot, emits:

- `type`: extreme-close-up | close-up | medium | wide | no-face
  (derived from average face-area-to-frame-area ratio across the shot)
- `motion`: static | slow-pan | fast-cut | handheld
  (derived from sparse optical-flow magnitude sampled inside the shot)

The shot list defines the time grid; this indexer just adds two
labels per shot. The agent uses these for editorial choices like
'find the cutaway b-rolls' (no-face + handheld) or 'pull the
intimate close-ups' (close-up + static).

Schema version: "1".
"""

from __future__ import annotations

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

_log = logging.getLogger(INDEXER_NAME)


server = IndexerServer(
    name=INDEXER_NAME,
    indexer_version=INDEXER_VERSION,
    schema_version=SCHEMA_VERSION,
)


def _project_root_from(asset_path: str) -> Path:
    """Walk up from the asset to the dir containing `index/`."""
    p = Path(asset_path).absolute()
    while p != p.parent:
        if (p / "index").is_dir():
            return p
        p = p.parent
    return Path(asset_path).absolute().parent


def _read_sidecar(project_root: Path, indexer: str, asset_id: str) -> dict[str, Any] | None:
    p = project_root / "index" / indexer / f"{asset_id}.json"
    if not p.exists():
        return None
    try:
        return json.loads(p.read_text())
    except (OSError, json.JSONDecodeError):
        return None


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
    return float(mag.mean())


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


@server.index_asset
def handle(req: IndexAssetRequest) -> dict[str, Any]:
    project_root = _project_root_from(req.asset_path)

    scenes_doc = _read_sidecar(project_root, "scenedetect", req.asset_id)
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

    face_doc = _read_sidecar(project_root, "face", req.asset_id)
    per_frame_faces = (
        face_doc.get("data", {}).get("per_frame", []) if face_doc else []
    )
    detect_w = int(face_doc.get("data", {}).get("detect_width", 0)) if face_doc else 0
    detect_h = int(face_doc.get("data", {}).get("detect_height", 0)) if face_doc else 0

    out_shots: list[dict[str, Any]] = []
    for shot in shots:
        start_s = float(shot["start_s"])
        end_s = float(shot["end_s"])
        face_ratio = _avg_face_ratio_in_window(
            per_frame_faces, detect_w, detect_h, start_s, end_s
        )
        shot_type = _classify_shot_type(face_ratio)
        # Sample optical flow at start, middle, end; pair adjacent samples.
        probes = max(2, FLOW_PROBES_PER_SHOT)
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
                    mags.append(_flow_magnitude(prev_frame, f))
                prev_frame = f
            mag = float(np.mean(mags)) if mags else 0.0
        motion = _bucket_motion(mag)

        out_shots.append(
            {
                "index": int(shot.get("index", len(out_shots))),
                "start_s": start_s,
                "end_s": end_s,
                "type": shot_type,
                "face_area_ratio": face_ratio,
                "motion": motion,
                "motion_magnitude": mag,
            }
        )

    _log.info(
        "shot-mcp: asset=%s shots=%d (face_data=%s)",
        req.asset_id,
        len(out_shots),
        bool(face_doc),
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
        "depends_on": ["scenedetect", "face"],
    }


def main() -> None:
    server.run()
