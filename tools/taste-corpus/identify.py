#!/usr/bin/env python3
"""Identify which published edit a raw recording corresponds to (or vice
versa) by ranked alignment coverage — content-based identification,
because drive naming lies (study Finding 13's pipeline-hygiene lesson).

The aligner doubles as a razor-sharp identifier: across TBPN
digest-live matrices, true pairs score 85-100% coverage while every
wrong pair scores 3-15%. This CLI formalizes that: align the query
transcript against every candidate and print the ranked coverages with
a confident/ambiguous verdict.

Usage:
  identify.py --published-vtt digest.vtt --candidates 'lives/*.vtt'
  identify.py --raw-vtt raw.vtt --candidates 'published/*.vtt'

Exactly one of --published-vtt / --raw-vtt: identification aligns
published→raw, so the query slots into whichever side it belongs.
"""

from __future__ import annotations

import argparse
import glob
import json
import sys

from align import align

CONFIDENT_COVERAGE = 0.5
AMBIGUITY_GAP = 0.2


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    side = ap.add_mutually_exclusive_group(required=True)
    side.add_argument("--published-vtt", help="query is a published edit")
    side.add_argument("--raw-vtt", help="query is a raw recording")
    ap.add_argument(
        "--candidates",
        required=True,
        help="glob of candidate VTTs for the other side",
    )
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    args = ap.parse_args()

    candidates = sorted(glob.glob(args.candidates))
    if not candidates:
        print(f"no candidates match {args.candidates!r}", file=sys.stderr)
        return 1

    ranked = []
    for candidate in candidates:
        if args.published_vtt:
            result = align(candidate, args.published_vtt)
        else:
            result = align(args.raw_vtt, candidate)
        ranked.append((result["alignment"]["coverage"], candidate))
    ranked.sort(reverse=True)

    best_cov, best_path = ranked[0]
    runner_up = ranked[1][0] if len(ranked) > 1 else 0.0
    confident = best_cov >= CONFIDENT_COVERAGE and (best_cov - runner_up) >= AMBIGUITY_GAP

    if args.json:
        print(
            json.dumps(
                {
                    "match": best_path if confident else None,
                    "confident": confident,
                    "ranked": [
                        {"candidate": path, "coverage": round(cov, 3)}
                        for cov, path in ranked
                    ],
                },
                indent=2,
            )
        )
    else:
        for cov, path in ranked:
            marker = " <-- match" if confident and path == best_path else ""
            print(f"{cov:6.2f}  {path}{marker}")
        if not confident:
            print(
                "no confident match (best "
                f"{best_cov:.0%}, runner-up {runner_up:.0%}; need >= "
                f"{CONFIDENT_COVERAGE:.0%} with a {AMBIGUITY_GAP:.0%} gap)",
                file=sys.stderr,
            )
    return 0 if confident else 2


if __name__ == "__main__":
    sys.exit(main())
