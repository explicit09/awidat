#!/usr/bin/env python3
"""Recover a professional editor's keep/cut/speed decisions by aligning a
raw recording's transcript against its published edit.

Productionized from the 2026-07-06 editorial study's prototype
(`doac-study/analysis/prototype/diff_own_fuzzy.py`): 15s windows both
sides, content-word TF-IDF, monotonic banded matching (each published
window matches the best raw window at-or-after the previous match, within
a lookahead band). Window-level cosine survives the cross-mix ASR drift
that breaks exact n-gram anchoring.

Emits a decision-list JSON per `decision_list_schema.json` — the contract
the montage-eval agreement scorer consumes. Study-calibrated noise
handling: auto-sub cue timestamps carry ~±0.1 jitter, so measured speed
ratios below `--speed-trust` (default 1.2) snap to 1.0; only sustained
readings survive as speed decisions.

Usage:
  align.py --raw-vtt raw.vtt --published-vtt pub.vtt \
           --pair-id brink-2026-05-02 --house technologia \
           --raw-ref "2026-05-02 14-42-27.mov" --published-ref Iv5_Udbilso \
           --out pairs/brink.json

Pure stdlib (no numpy) so it runs anywhere python3 does.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
from collections import Counter

WINDOW_S = 15.0
MIN_SIM = 0.18
LOOKBACK = 2
LOOKAHEAD = 40  # windows -> 10 min band at 15s windows
MIN_CUT_S = 45.0
SPEED_TRUST = 1.2
MAX_MISSES_BEFORE_UNLOCK = 3
MAX_PUB_STEP = 4  # consecutive-match spans measurable for speed
MAX_RAW_STEP = 8

STOP = set(
    """a an the and or but if then than so as of in on at to for with
by from up down out over under again is are was were be been being have has
had do does did will would can could should may might must this that these
those i you he she it we they them his her its our their what which who whom
not no nor only own same too very just also there here when where why how all
any both each few more most other some such yeah like know right really going
think want cause gonna kind lot say said says get got mean thing things""".split()
)

TIMESTAMP = re.compile(r"(\d+):(\d+):(\d+)\.(\d+) -->")
TAG = re.compile(r"<[^>]+>")
WORD = re.compile(r"[a-z']{3,}")


def vtt_cues(path: str) -> list[tuple[float, str]]:
    """Parse a VTT into (start_seconds, lowercased text) cues."""
    with open(path, encoding="utf-8") as f:
        content = f.read()
    cues = []
    for block in re.split(r"\n\n+", content):
        m = TIMESTAMP.search(block)
        if not m:
            continue
        secs = (
            int(m.group(1)) * 3600
            + int(m.group(2)) * 60
            + int(m.group(3))
            + int(m.group(4)) / 1000
        )
        text = TAG.sub("", block)
        text = " ".join(
            line.strip()
            for line in text.splitlines()
            if line.strip() and "-->" not in line and not line.strip().isdigit()
        )
        text = re.sub(r"\s+", " ", text).strip().lower()
        if text:
            cues.append((secs, text))
    return cues


def window_bags(cues: list[tuple[float, str]], window_s: float) -> list[Counter]:
    """Bucket content words into fixed windows."""
    if not cues:
        return []
    duration = cues[-1][0] + window_s
    buckets = [Counter() for _ in range(int(duration // window_s) + 1)]
    for t, text in cues:
        words = [w for w in WORD.findall(text) if w not in STOP]
        buckets[int(t // window_s)].update(words)
    return buckets


def match_windows(
    raw_bags: list[Counter], pub_bags: list[Counter]
) -> list[tuple[int, int, float]]:
    """Monotonic banded best-match: (pub_idx, raw_idx, cosine)."""
    df = Counter(w for bag in raw_bags for w in set(bag))
    n = max(len(raw_bags), 1)
    idf = {w: math.log(n / (1 + c)) for w, c in df.items()}

    def vec(bag: Counter) -> dict[str, float]:
        return {w: c * idf.get(w, 0.5) for w, c in bag.items()}

    def cos(a: dict[str, float], b: dict[str, float]) -> float:
        if not a or not b:
            return 0.0
        num = sum(v * b.get(w, 0.0) for w, v in a.items())
        na = math.sqrt(sum(v * v for v in a.values()))
        nb = math.sqrt(sum(v * v for v in b.values()))
        return num / (na * nb) if na and nb else 0.0

    raw_vecs = [vec(bag) for bag in raw_bags]
    matches: list[tuple[int, int, float]] = []
    last = 0
    locked = False
    misses = 0
    for pi, bag in enumerate(pub_bags):
        pv = vec(bag)
        if not pv:
            continue
        # Banded search while locked (enforces monotonic locality);
        # GLOBAL search from `last` when unlocked — the first published
        # window may map deep into the raw (a 24-minute pre-show discard
        # broke the always-banded prototype: no match within the 10-min
        # band, lock never acquired, 8% coverage on a pair the study
        # aligned at 95%). Losing the lock mid-show (structural jump
        # longer than the band) re-triggers global search the same way.
        if locked:
            lo = max(0, last - LOOKBACK)
            hi = min(len(raw_vecs), last + LOOKAHEAD)
        else:
            lo, hi = last, len(raw_vecs)
        best, best_ri = 0.0, -1
        for ri in range(lo, hi):
            s = cos(pv, raw_vecs[ri])
            if s > best:
                best, best_ri = s, ri
        if best >= MIN_SIM and best_ri >= 0:
            matches.append((pi, best_ri, best))
            last = best_ri + 1
            locked = True
            misses = 0
        else:
            misses += 1
            if misses > MAX_MISSES_BEFORE_UNLOCK:
                locked = False
    return matches


def build_segments(
    matches: list[tuple[int, int, float]],
    raw_windows: int,
    window_s: float,
    speed_trust: float,
) -> list[dict]:
    """Merge window matches into keep runs (with speed) and cut gaps.

    A keep run is a maximal chain of consecutive matches whose pub/raw
    steps are small enough to measure (study: dp<=4, dr<=8). The run's
    speed = raw-span / pub-span, snapped to 1.0 below the trust
    threshold. Raw coverage gaps >= MIN_CUT_S become cut segments,
    scoped pre_show / in_show / post_show.
    """
    segments: list[dict] = []
    if not matches:
        if raw_windows:
            segments.append(
                {
                    "action": "cut",
                    "raw_span": [0.0, raw_windows * window_s],
                    "scope": "pre_show",
                }
            )
        return segments

    # Keep runs.
    runs: list[list[tuple[int, int, float]]] = [[matches[0]]]
    for prev, cur in zip(matches, matches[1:]):
        dp = cur[0] - prev[0]
        dr = cur[1] - prev[1]
        if 0 < dp <= MAX_PUB_STEP and 0 < dr <= MAX_RAW_STEP:
            runs[-1].append(cur)
        else:
            runs.append([cur])

    for run in runs:
        p0, r0, _ = run[0]
        p1, r1, _ = run[-1]
        raw_span = [r0 * window_s, (r1 + 1) * window_s]
        pub_span = [p0 * window_s, (p1 + 1) * window_s]
        raw_len = raw_span[1] - raw_span[0]
        pub_len = pub_span[1] - pub_span[0]
        ratio = raw_len / pub_len if pub_len > 0 else 1.0
        speed = round(ratio, 2) if ratio >= speed_trust else 1.0
        confidence = round(sum(m[2] for m in run) / len(run), 3)
        segments.append(
            {
                "action": "keep",
                "raw_span": raw_span,
                "published_span": pub_span,
                "speed": speed,
                "confidence": confidence,
            }
        )

    # Cut gaps in raw coverage.
    first_kept = matches[0][1] * window_s
    last_kept = (matches[-1][1] + 1) * window_s
    covered = [False] * raw_windows
    for _, ri, _ in matches:
        covered[ri] = True

    def push_cut(start_w: int, end_w: int) -> None:
        a, b = start_w * window_s, end_w * window_s
        if b - a < MIN_CUT_S:
            return
        if b <= first_kept:
            scope = "pre_show"
        elif a >= last_kept:
            scope = "post_show"
        else:
            scope = "in_show"
        segments.append({"action": "cut", "raw_span": [a, b], "scope": scope})

    gap_start = None
    for w in range(raw_windows):
        if not covered[w]:
            if gap_start is None:
                gap_start = w
        else:
            if gap_start is not None:
                push_cut(gap_start, w)
            gap_start = None
    if gap_start is not None:
        push_cut(gap_start, raw_windows)

    segments.sort(key=lambda s: s["raw_span"][0])
    return segments


def align(raw_vtt: str, published_vtt: str, speed_trust: float = SPEED_TRUST) -> dict:
    raw_bags = window_bags(vtt_cues(raw_vtt), WINDOW_S)
    pub_bags = window_bags(vtt_cues(published_vtt), WINDOW_S)
    matches = match_windows(raw_bags, pub_bags)
    pub_nonempty = sum(1 for bag in pub_bags if bag)
    segments = build_segments(matches, len(raw_bags), WINDOW_S, speed_trust)
    return {
        "window_s": WINDOW_S,
        "alignment": {
            "published_windows": pub_nonempty,
            "matched_windows": len(matches),
            "coverage": round(len(matches) / pub_nonempty, 3) if pub_nonempty else 0.0,
        },
        "segments": segments,
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--raw-vtt", required=True)
    ap.add_argument("--published-vtt", required=True)
    ap.add_argument("--pair-id", required=True)
    ap.add_argument("--house", required=True)
    ap.add_argument("--raw-ref", required=True)
    ap.add_argument("--published-ref", required=True)
    ap.add_argument("--speed-trust", type=float, default=SPEED_TRUST)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    result = align(args.raw_vtt, args.published_vtt, args.speed_trust)
    doc = {
        "version": 1,
        "pair_id": args.pair_id,
        "house": args.house,
        "raw_ref": args.raw_ref,
        "published_ref": args.published_ref,
        **result,
    }
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(doc, f, indent=2)
        f.write("\n")
    cov = doc["alignment"]["coverage"]
    keeps = sum(1 for s in doc["segments"] if s["action"] == "keep")
    cuts = sum(1 for s in doc["segments"] if s["action"] == "cut")
    sped = sum(1 for s in doc["segments"] if s.get("speed", 1.0) > 1.0)
    print(
        f"{args.pair_id}: coverage {cov:.0%}, {keeps} keep run(s) "
        f"({sped} sped), {cuts} cut region(s) -> {args.out}"
    )
    if cov < 0.5:
        print(
            "warning: coverage < 50% — wrong pair, heavy reordering, or ASR "
            "mismatch; verify before trusting these labels",
            file=sys.stderr,
        )
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
