#!/usr/bin/env python3
"""Summarize color-analysis indexes for offline correction benchmarking.

The script emits JSON and never edits the project. Optional datasets stay
outside the repo; this helper only records what was evaluated and what the
current deterministic analyzer would recommend.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


def load_index(path: Path) -> dict[str, Any]:
    raw = json.loads(path.read_text())
    data = raw.get("data", raw)
    summary = data.get("summary", {})
    scenes = data.get("scenes", [])
    return {
        "path": str(path),
        "frame_count": data.get("frame_count", 0),
        "issue_tags": summary.get("issue_tags", []),
        "auto_correct_safe": bool(summary.get("auto_correct_safe", False)),
        "confidence": float(summary.get("confidence", 0.0)),
        "policy": summary.get("policy", {}),
        "recommended_correction": summary.get("recommended_correction", {}),
        "scene_count": len(scenes),
        "scenes": [
            {
                "scene_id": s.get("scene_id"),
                "start_s": s.get("start_s"),
                "end_s": s.get("end_s"),
                "issue_tags": s.get("issue_tags", []),
                "auto_correct_safe": bool(s.get("auto_correct_safe", False)),
                "confidence": float(s.get("confidence", 0.0)),
                "policy": s.get("policy", {}),
                "recommended_correction": s.get("recommended_correction", {}),
            }
            for s in scenes
        ],
    }


def aggregate(rows: list[dict[str, Any]]) -> dict[str, Any]:
    if not rows:
        return {"index_count": 0, "safe_count": 0, "mean_confidence": 0.0, "issue_counts": {}}
    issue_counts: dict[str, int] = {}
    for row in rows:
        for tag in row["issue_tags"]:
            issue_counts[tag] = issue_counts.get(tag, 0) + 1
    action_counts: dict[str, int] = {}
    for row in rows:
        action = row.get("policy", {}).get("recommended_action", "unknown")
        action_counts[action] = action_counts.get(action, 0) + 1
    return {
        "index_count": len(rows),
        "safe_count": sum(1 for row in rows if row["auto_correct_safe"]),
        "mean_confidence": sum(row["confidence"] for row in rows) / len(rows),
        "issue_counts": issue_counts,
        "action_counts": action_counts,
    }


def discover_indexes(dataset_dir: str | None) -> list[Path]:
    if not dataset_dir:
        return []
    root = Path(dataset_dir)
    if not root.exists():
        return []
    candidates = []
    for path in root.rglob("*.json"):
        lower_path = str(path).lower()
        if "color-analysis" in lower_path or "colour-analysis" in lower_path:
            candidates.append(path)
            continue
        try:
            raw = json.loads(path.read_text())
        except (OSError, json.JSONDecodeError):
            continue
        data = raw.get("data", raw)
        if data.get("summary", {}).get("recommended_correction") is not None:
            candidates.append(path)
    return sorted(candidates)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--color-index", action="append", default=[], help="Path to read_index(channel=color) JSON")
    parser.add_argument("--dataset-dir", help="Optional external dataset/cache directory label")
    parser.add_argument("--output-json", help="Optional path to write the same JSON report")
    args = parser.parse_args()

    paths = [Path(p) for p in args.color_index]
    paths.extend(path for path in discover_indexes(args.dataset_dir) if path not in paths)
    indexes = [load_index(path) for path in paths]
    report = {
        "status": "passed" if indexes else "empty",
        "dataset_dir": args.dataset_dir,
        "indexes": indexes,
        "aggregate": aggregate(indexes),
        "runtime_dependency": "none; datasets are offline benchmark inputs only",
    }
    if args.output_json:
        Path(args.output_json).parent.mkdir(parents=True, exist_ok=True)
        Path(args.output_json).write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
