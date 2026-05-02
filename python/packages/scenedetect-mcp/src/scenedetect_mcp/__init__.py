"""Shot-boundary indexer for Awidat using PySceneDetect.

Emits `shots[]` — a list of contiguous shots, each with `start_s` /
`end_s`. The agent uses this for "where could a cut land that doesn't
cross a shot boundary."

Detector: ContentDetector (color-difference based). Threshold 27.0 is
PySceneDetect's documented default and works well on talking-head video.
For users with rapid-montage content, a TransNetV2 backend can be slotted
in as a separate indexer (`shots-transnet`) without engine changes — the
sidecar contract is the same.

Schema version: "1".
"""

from __future__ import annotations

from typing import Any

from scenedetect import ContentDetector, SceneManager, open_video

from awidat_mcp import IndexAssetRequest, IndexerServer

INDEXER_NAME = "scenedetect"
INDEXER_VERSION = "0.1.0"
SCHEMA_VERSION = "1"

# PySceneDetect's documented default. Higher = fewer (only big) cuts.
THRESHOLD = 27.0

# Min shot length: shots shorter than this are likely false positives
# from compression artifacts.
MIN_SHOT_S = 0.4


server = IndexerServer(
    name=INDEXER_NAME,
    indexer_version=INDEXER_VERSION,
    schema_version=SCHEMA_VERSION,
)


@server.index_asset
def handle(req: IndexAssetRequest) -> dict[str, Any]:
    video = open_video(req.asset_path)
    sm = SceneManager()
    sm.add_detector(ContentDetector(threshold=THRESHOLD, min_scene_len=15))
    sm.detect_scenes(video=video, show_progress=False)
    scenes = sm.get_scene_list()

    shots: list[dict[str, float | int]] = []
    for idx, (start, end) in enumerate(scenes):
        start_s = float(start.get_seconds())
        end_s = float(end.get_seconds())
        if end_s - start_s < MIN_SHOT_S:
            continue
        shots.append(
            {
                "index": idx,
                "start_s": start_s,
                "end_s": end_s,
                "start_frame": int(start.get_frames()),
                "end_frame": int(end.get_frames()),
            }
        )

    return {
        "frame_rate": float(video.frame_rate),
        "duration_s": float(video.duration.get_seconds()),
        "threshold": THRESHOLD,
        "shots": shots,
    }


def main() -> None:
    server.run()
