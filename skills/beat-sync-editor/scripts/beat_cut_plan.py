#!/usr/bin/env python3
"""Generate beat-aligned cut targets from a beat JSON file."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def load_body(path: str) -> dict:
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def beat_times(body: dict) -> list[float]:
    raw = body.get("beats", body.get("beat_times", []))
    out = []
    for item in raw:
        if isinstance(item, dict):
            out.append(float(item.get("time_s", item.get("start_s", 0.0))))
        else:
            out.append(float(item))
    return sorted(out)


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--beats", required=True)
    p.add_argument("--shot")
    p.add_argument("--cut-every", type=int, default=4)
    p.add_argument("--duration-s", type=float)
    args = p.parse_args()

    beats = beat_times(load_body(args.beats))
    shot = load_body(args.shot) if args.shot else {}
    cuts = []
    for i, t in enumerate(beats):
        if args.duration_s is not None and t > args.duration_s:
            break
        if i % max(1, args.cut_every) == 0:
            cuts.append({
                "target_s": round(t, 3),
                "beat_index": i,
                "near_motion": motion_near(shot, t),
                "transition": "none" if i else "awidat.fade_in",
                "tolerance_ms": 50,
            })
    print(json.dumps({"cut_every": args.cut_every, "cuts": cuts}, indent=2))


def motion_near(shot: dict, t: float) -> bool:
    for s in shot.get("shots", []):
        if float(s.get("start_s", 0)) <= t <= float(s.get("end_s", 0)):
            return s.get("motion") in {"slow-pan", "handheld", "fast-cut"}
    return False


if __name__ == "__main__":
    main()
