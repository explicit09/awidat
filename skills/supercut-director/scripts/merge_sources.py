#!/usr/bin/env python3
"""Merge per-source clip candidates into one cross-source supercut selection.

This is the ONE piece of logic single-asset skills don't have: ranking and
balancing standalone moments ACROSS many source episodes. It does NOT score
moments itself — run viral-clip-extractor's ``score_moments.py`` once per
source, tag each result with its asset id and (optionally) a speaker id, then
feed the per-source candidate files here.

Input: one or more ``--source`` files. Each is either

    {"asset": "raw/ep-01.mp4", "candidates": [ ... score_moments output ... ]}

or the bare ``{"candidates": [...]}`` emitted by score_moments.py, in which
case pass ``--asset`` is read from a sibling ``--source-asset`` mapping is not
needed — instead wrap it (see ``--source`` form above) or pass the asset id in
the filename via ``name=path`` (e.g. ``--source raw/ep-01.mp4=cand-01.json``).

The merger applies a global "cold-click" quality bar, an optional per-speaker
balance target, and a per-source cap so no single episode dominates, then
returns a globally ranked, deduplicated selection.
"""

from __future__ import annotations

import argparse
import json
from collections import Counter, defaultdict
from pathlib import Path


def load_source(spec: str) -> tuple[str, list[dict]]:
    """Parse a ``--source`` spec into (asset_id, candidates)."""
    asset_override = None
    path = spec
    if "=" in spec:
        asset_override, path = spec.split("=", 1)
    raw = json.loads(Path(path).read_text())
    body = raw.get("data", raw)
    asset = asset_override or body.get("asset") or body.get("asset_id")
    candidates = body.get("candidates", body.get("results", []))
    if not asset:
        raise ValueError(
            f"source {path!r} has no asset id; wrap it as "
            f'{{"asset": "...", "candidates": [...]}} or pass '
            f"--source asset_id=path.json"
        )
    return str(asset), list(candidates)


def speaker_of(candidate: dict) -> str:
    """Diarized speaker label, if score_moments was fed a diarized transcript."""
    return str(
        candidate.get("speaker_id")
        or candidate.get("speaker")
        or "unknown"
    )


def select(
    sources: list[tuple[str, list[dict]]],
    *,
    min_score: float,
    target_count: int,
    per_source_cap: int | None,
    balance_speakers: bool,
) -> dict:
    pool: list[dict] = []
    for asset, candidates in sources:
        for c in candidates:
            if float(c.get("score", 0.0)) < min_score:
                continue
            entry = dict(c)
            entry["asset"] = asset
            entry["speaker_id"] = speaker_of(c)
            pool.append(entry)

    # Highest score first; this is the global cold-click ranking.
    pool.sort(key=lambda c: float(c.get("score", 0.0)), reverse=True)

    selected: list[dict] = []
    per_source: Counter = Counter()
    per_speaker: Counter = Counter()
    rejected_below_bar = sum(
        1
        for _, cands in sources
        for c in cands
        if float(c.get("score", 0.0)) < min_score
    )

    # Quota is based on the *eligible* speakers (those with at least one
    # above-bar clip in `pool`), not every speaker in the raw candidate set.
    # Counting ineligible speakers would lower the ceiling and starve speakers
    # who do have eligible clips.
    eligible_speakers = {c["speaker_id"] for c in pool}
    eligible_speakers.discard("unknown")

    def speaker_quota_ok(spk: str) -> bool:
        if not balance_speakers or not eligible_speakers:
            return True
        # Soft cap: no speaker may exceed ceil(target / num_speakers) + 1.
        ceiling = -(-target_count // max(1, len(eligible_speakers))) + 1
        return per_speaker[spk] < ceiling

    # Track already-claimed moments so overlapping variants of the same source
    # moment (same moment_id, or same asset + time range) can't fill multiple
    # spine slots and make the supercut repeat a clip.
    seen_moments: set = set()

    def moment_key(c: dict) -> object:
        mid = c.get("moment_id")
        if mid is not None:
            return ("mid", mid)
        return (
            "range",
            c["asset"],
            round(float(c.get("start_s", 0.0)), 2),
            round(float(c.get("end_s", 0.0)), 2),
        )

    for c in pool:
        if len(selected) >= target_count:
            break
        asset = c["asset"]
        spk = c["speaker_id"]
        key = moment_key(c)
        if key in seen_moments:
            continue
        if per_source_cap is not None and per_source[asset] >= per_source_cap:
            continue
        if not speaker_quota_ok(spk):
            continue
        selected.append(c)
        seen_moments.add(key)
        per_source[asset] += 1
        per_speaker[spk] += 1

    # Spine ordering: hook-first, then descending score. A "hook" is any
    # candidate whose kind is hook or whose hook_signal flag is set.
    def is_hook(c: dict) -> bool:
        return c.get("kind") == "hook" or bool(c.get("hook_signal"))

    spine = sorted(
        selected,
        key=lambda c: (0 if is_hook(c) else 1, -float(c.get("score", 0.0))),
    )
    for i, c in enumerate(spine):
        c["spine_position"] = i

    return {
        "spine": spine,
        "stats": {
            "sources": len(sources),
            "pool_after_bar": len(pool),
            "rejected_below_bar": rejected_below_bar,
            "selected": len(spine),
            "per_source": dict(per_source),
            "per_speaker": dict(per_speaker),
            "min_score": min_score,
            "balanced_speakers": balance_speakers,
        },
    }


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument(
        "--source",
        action="append",
        required=True,
        metavar="[asset_id=]candidates.json",
        help="Per-source score_moments output. Repeat for each episode.",
    )
    p.add_argument(
        "--min-score",
        type=float,
        default=60.0,
        help="Cold-click quality bar. Candidates below this are dropped.",
    )
    p.add_argument("--target-count", type=int, default=12)
    p.add_argument(
        "--per-source-cap",
        type=int,
        default=None,
        help="Max clips kept from any one episode (None = unlimited).",
    )
    p.add_argument(
        "--balance-speakers",
        action="store_true",
        help="Spread selection across diarized speakers when speaker_id present.",
    )
    args = p.parse_args()

    sources = [load_source(spec) for spec in args.source]
    result = select(
        sources,
        min_score=args.min_score,
        target_count=args.target_count,
        per_source_cap=args.per_source_cap,
        balance_speakers=args.balance_speakers,
    )
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
