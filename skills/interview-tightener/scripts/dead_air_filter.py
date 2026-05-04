#!/usr/bin/env python3
"""
Find trim candidates from an audio-energy sidecar.

Usage:
  python3 dead_air_filter.py <path/to/audio-energy/<asset>.json> \\
      [--min-dead-air-s 1.0] [--min-cluster-window-s 1.0] \\
      [--cluster-threshold 2]

Output: JSON list to stdout, one entry per trim candidate:
    {"start_s": ..., "end_s": ..., "duration_s": ..., "reason": ...}

Reasons:
  - dead_air: contiguous below-threshold energy >= --min-dead-air-s
  - filler_cluster: NOT detected from audio-energy alone (this script
    handles dead air only); the agent finds filler clusters via
    transcript scanning. Listed in the SKILL.md as a pipeline step,
    but it's a Rust-side concern, not this script's.

The skill-md's "the bash script returns dead air + filler" is a
slight simplification — this script returns dead-air candidates
only. The interview-tightener skill is the orchestrator; it pairs
this script's output with `find_moment` results for filler hunting.

We deliberately keep this script narrow. Anything that needs a
sentence-transformer or a transcript walk lives in the agent's
tool layer, not in a SKILL.md script. Scripts should be
deterministic + offline + cheap.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any


def find_dead_air(
    energy_samples: list[dict[str, Any]],
    min_dead_air_s: float,
    silence_db: float,
) -> list[dict[str, Any]]:
    """Walk a flat list of {t_s, db} samples; return contiguous runs
    below `silence_db` lasting >= `min_dead_air_s`."""
    cuts: list[dict[str, Any]] = []
    run_start: float | None = None
    last_t: float = 0.0
    for s in energy_samples:
        t = float(s.get("t_s", 0.0))
        db = float(s.get("db", 0.0))
        if db <= silence_db:
            if run_start is None:
                run_start = t
            last_t = t
        else:
            if run_start is not None and (last_t - run_start) >= min_dead_air_s:
                cuts.append(
                    {
                        "start_s": round(run_start, 3),
                        "end_s": round(last_t, 3),
                        "duration_s": round(last_t - run_start, 3),
                        "reason": "dead_air",
                    }
                )
            run_start = None
    # Trailing run.
    if run_start is not None and (last_t - run_start) >= min_dead_air_s:
        cuts.append(
            {
                "start_s": round(run_start, 3),
                "end_s": round(last_t, 3),
                "duration_s": round(last_t - run_start, 3),
                "reason": "dead_air",
            }
        )
    return cuts


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("sidecar", help="path to <project>/index/audio-energy/<asset>.json")
    p.add_argument("--min-dead-air-s", type=float, default=1.0)
    p.add_argument("--silence-db", type=float, default=-30.0,
                   help="dBFS threshold for 'silence'. -30 catches a typical interview studio's noise floor.")
    args = p.parse_args()

    sidecar_path = Path(args.sidecar)
    if not sidecar_path.exists():
        print(f"sidecar not found: {sidecar_path}", file=sys.stderr)
        return 2

    body = json.loads(sidecar_path.read_text())
    samples = body.get("data", {}).get("samples") or body.get("samples")
    if not isinstance(samples, list):
        print(f"sidecar has no 'samples' array; not an audio-energy sidecar?",
              file=sys.stderr)
        return 2

    cuts = find_dead_air(
        samples,
        min_dead_air_s=args.min_dead_air_s,
        silence_db=args.silence_db,
    )
    cuts.sort(key=lambda c: c["duration_s"], reverse=True)
    json.dump(cuts, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
