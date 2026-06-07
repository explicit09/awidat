#!/usr/bin/env python3
"""Generate beat-synced speed-ramp effect plans."""

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
            value = item.get("time_s", item.get("start_s", 0.0))
        else:
            value = item
        try:
            out.append(float(value))
        except (TypeError, ValueError):
            continue
    return sorted(time for time in out if time >= 0.0)


def plan_speed_ramps(
    *,
    beats: list[float],
    duration_s: float,
    accent_every: int,
    slow_factor: float,
    fast_factor: float,
    ramp_window_s: float,
) -> dict:
    if duration_s <= 0.0:
        raise ValueError("duration_s must be > 0")
    if slow_factor <= 0.0 or fast_factor <= 0.0:
        raise ValueError("speed factors must be > 0")
    if ramp_window_s < 0.0:
        raise ValueError("ramp_window_s must be >= 0")

    scoped_beats = [time for time in sorted(beats) if 0.0 <= time <= duration_s]
    cadence = max(1, accent_every)
    accent_indices = [idx for idx, _ in enumerate(scoped_beats) if idx % cadence == 0]
    speed_points = build_speed_points(
        scoped_beats=scoped_beats,
        accent_indices=accent_indices,
        duration_s=duration_s,
        slow_factor=slow_factor,
        fast_factor=fast_factor,
        ramp_window_s=ramp_window_s,
    )

    return {
        "effect": "montage.time_remap",
        "accent_beats": accent_indices,
        "nominal_speed": 1.0,
        "speed_points": [
            {"time_s": round(time, 3), "factor": round(factor, 3)}
            for time, factor in speed_points
        ],
        "curve": build_time_remap_curve(speed_points, duration_s),
    }


def build_speed_points(
    *,
    scoped_beats: list[float],
    accent_indices: list[int],
    duration_s: float,
    slow_factor: float,
    fast_factor: float,
    ramp_window_s: float,
) -> list[tuple[float, float]]:
    by_time: dict[float, float] = {0.0: 1.0}
    for idx in accent_indices:
        beat_time = scoped_beats[idx]
        by_time[beat_time] = min(by_time.get(beat_time, slow_factor), slow_factor)
        recovery_time = min(duration_s, beat_time + ramp_window_s)
        if recovery_time > beat_time:
            by_time[recovery_time] = max(by_time.get(recovery_time, fast_factor), fast_factor)
    by_time[duration_s] = by_time.get(duration_s, 1.0)
    return sorted(by_time.items())


def build_time_remap_curve(speed_points: list[tuple[float, float]], duration_s: float) -> list[dict]:
    curve = [{"source_time_s": 0.0, "timeline_time_s": 0.0}]
    timeline_time_s = 0.0
    previous_time = 0.0
    previous_factor = speed_points[0][1] if speed_points else 1.0

    for time_s, factor in speed_points[1:]:
        if time_s <= previous_time:
            previous_factor = factor
            continue
        timeline_time_s += (time_s - previous_time) / previous_factor
        curve.append(
            {
                "source_time_s": round(time_s, 3),
                "timeline_time_s": round(timeline_time_s, 3),
            }
        )
        previous_time = time_s
        previous_factor = factor

    if curve[-1]["source_time_s"] < duration_s:
        timeline_time_s += (duration_s - previous_time) / previous_factor
        curve.append(
            {
                "source_time_s": round(duration_s, 3),
                "timeline_time_s": round(timeline_time_s, 3),
            }
        )
    return curve


def format_set_effect_edl(*, anchor: str, plan: dict, rationale: str | None = None) -> str:
    if plan.get("effect") != "montage.time_remap":
        raise ValueError("plan effect must be montage.time_remap")
    curve = plan.get("curve")
    if not isinstance(curve, list) or len(curve) < 2:
        raise ValueError("plan curve must contain at least two points")

    lines = [
        "*** Set Effect",
        f"@@ anchor: {anchor}",
        "+ effect: montage.time_remap",
        f"+ params_json: {json.dumps({'curve': curve})}",
    ]
    if rationale:
        lines.append(f"+ rationale: {rationale}")
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--beats", required=True)
    parser.add_argument("--duration-s", required=True, type=float)
    parser.add_argument("--accent-every", type=int, default=4)
    parser.add_argument("--slow-factor", type=float, default=0.4)
    parser.add_argument("--fast-factor", type=float, default=1.3)
    parser.add_argument("--ramp-window-s", type=float, default=0.18)
    parser.add_argument("--edl-anchor")
    parser.add_argument("--rationale")
    args = parser.parse_args()

    plan = plan_speed_ramps(
        beats=beat_times(load_body(args.beats)),
        duration_s=args.duration_s,
        accent_every=args.accent_every,
        slow_factor=args.slow_factor,
        fast_factor=args.fast_factor,
        ramp_window_s=args.ramp_window_s,
    )
    if args.edl_anchor:
        print(format_set_effect_edl(anchor=args.edl_anchor, plan=plan, rationale=args.rationale))
    else:
        print(json.dumps(plan, indent=2))


if __name__ == "__main__":
    main()
