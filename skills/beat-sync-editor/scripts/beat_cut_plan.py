#!/usr/bin/env python3
"""Generate beat-aligned cut targets from a beat JSON file."""

from __future__ import annotations

import argparse
import json
import math
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


def plan_cuts(
    *,
    beats: list[float],
    cut_every: int,
    duration_s: float | None,
    shot: dict,
    audio_energy: dict,
) -> list[dict]:
    scoped_beats = [
        t for t in beats if duration_s is None or t <= duration_s
    ]
    if not scoped_beats:
        return []

    energy_windows = energy_window_scores(audio_energy)
    if energy_windows:
        return energy_ranked_cuts(
            beats=scoped_beats,
            cut_every=cut_every,
            shot=shot,
            energy_windows=energy_windows,
        )
    return cadence_cuts(scoped_beats, cut_every=cut_every, shot=shot)


def cadence_cuts(beats: list[float], *, cut_every: int, shot: dict) -> list[dict]:
    cuts = []
    for i, t in enumerate(beats):
        if i % max(1, cut_every) != 0:
            continue
        cuts.append(cut_payload(t, i, shot, selection_reason="cadence"))
    return cuts


def energy_ranked_cuts(
    *,
    beats: list[float],
    cut_every: int,
    shot: dict,
    energy_windows: list[tuple[float, float]],
) -> list[dict]:
    target_count = max(1, math.ceil(len(beats) / max(1, cut_every)))
    scored = []
    all_scores = [score for _, score in energy_windows]
    quiet_floor_db = percentile(all_scores, 35.0)

    for i, t in enumerate(beats):
        score = nearest_energy_score(energy_windows, t)
        if score is None:
            scored.append((i, t, quiet_floor_db, "cadence"))
            continue
        reason = "energy_peak" if score > quiet_floor_db else "cadence"
        scored.append((i, t, score, reason))

    energetic = [item for item in scored if item[3] == "energy_peak"]
    if len(energetic) >= target_count:
        selected = sorted(energetic, key=lambda item: item[2], reverse=True)[:target_count]
    else:
        selected = sorted(scored, key=lambda item: item[2], reverse=True)[:target_count]

    selected.sort(key=lambda item: item[0])
    return [
        cut_payload(
            t,
            i,
            shot,
            selection_reason=reason,
            energy_score=score,
            quiet_floor_db=quiet_floor_db,
        )
        for i, t, score, reason in selected
    ]


def energy_window_scores(audio_energy: dict) -> list[tuple[float, float]]:
    windows = []
    for item in audio_energy.get("windows", []):
        try:
            windows.append((float(item.get("start_s", 0.0)), float(item.get("rms_db", -80.0))))
        except (TypeError, ValueError):
            continue
    return sorted(windows)


def nearest_energy_score(windows: list[tuple[float, float]], t: float) -> float | None:
    if not windows:
        return None
    nearest_time, nearest_score = min(windows, key=lambda item: abs(item[0] - t))
    if abs(nearest_time - t) > 0.15:
        return None
    return nearest_score


def percentile(values: list[float], pct: float) -> float:
    if not values:
        return -80.0
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    pos = (len(ordered) - 1) * max(0.0, min(100.0, pct)) / 100.0
    lower = math.floor(pos)
    upper = math.ceil(pos)
    if lower == upper:
        return ordered[int(pos)]
    weight = pos - lower
    return ordered[lower] * (1.0 - weight) + ordered[upper] * weight


def cut_payload(
    t: float,
    beat_index: int,
    shot: dict,
    *,
    selection_reason: str,
    energy_score: float | None = None,
    quiet_floor_db: float | None = None,
) -> dict:
    payload = {
        "target_s": round(t, 3),
        "beat_index": beat_index,
        "near_motion": motion_near(shot, t),
        "transition": "none",
        "tolerance_ms": 50,
        "selection_reason": selection_reason,
    }
    if energy_score is not None:
        payload["energy_score"] = round(energy_score, 3)
    if quiet_floor_db is not None:
        payload["quiet_floor_db"] = round(quiet_floor_db, 3)
    return payload


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--beats", required=True)
    p.add_argument("--audio-energy")
    p.add_argument("--shot")
    p.add_argument("--cut-every", type=int, default=4)
    p.add_argument("--duration-s", type=float)
    args = p.parse_args()

    beats = beat_times(load_body(args.beats))
    shot = load_body(args.shot) if args.shot else {}
    audio_energy = load_body(args.audio_energy) if args.audio_energy else {}
    cuts = plan_cuts(
        beats=beats,
        cut_every=args.cut_every,
        duration_s=args.duration_s,
        shot=shot,
        audio_energy=audio_energy,
    )
    print(json.dumps({"cut_every": args.cut_every, "cuts": cuts}, indent=2))


def motion_near(shot: dict, t: float) -> bool:
    for s in shot.get("shots", []):
        if float(s.get("start_s", 0)) <= t <= float(s.get("end_s", 0)):
            return s.get("motion") in {"slow-pan", "handheld", "fast-cut"}
    return False


if __name__ == "__main__":
    main()
