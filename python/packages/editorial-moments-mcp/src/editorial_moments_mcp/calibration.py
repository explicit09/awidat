"""Calibration metrics for editorial-moments scoring.

Why this exists
---------------
The editorial-moments indexer assigns 0..1 scores to typed beats
(hook, punchline, emotional_peak, …). Montage's claim is that these
multimodal scores predict human-rated editorial importance better
than an LLM-only highlight detector. That claim is only defensible
if we measure it. This module is the measurement.

What we compute
---------------
Given a list of clips with both predicted and ground-truth per-kind
scores, we emit three metrics per kind:

* `pearson_correlation` — linear correlation between predicted and
  human-labelled scores. Detects monotone agreement after centering.
* `top_k_agreement` — of the top-K human-rated clips, what fraction
  appear in the top-K predicted? A ranking metric — robust to
  monotone score-distortions Pearson would penalise.
* `calibration_bins` — bin predictions into N equal-width buckets in
  [0, 1] and report the mean ground-truth label inside each bucket.
  This is the data behind a reliability diagram (a la scikit-learn's
  `calibration_curve`): a well-calibrated scorer's `label_mean`
  should track its `pred_mean` along the y=x line.

No model retraining here — we only score the scorer.

Why it lives in the package, not in /scripts
--------------------------------------------
The metrics are reusable: an eval CI job, a notebook, and the
`calibrate.py` CLI all want the same Pearson + top-K + bin code.
Per the project's DRY/SRP rules, the formulas belong in the package;
the CLI just orchestrates them.
"""

from __future__ import annotations

import json
import math
import statistics
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence


# ---------------------------------------------------------------------------
# Pure metric primitives.
# ---------------------------------------------------------------------------


def pearson_correlation(
    predictions: Sequence[float], labels: Sequence[float]
) -> float:
    """Pearson product-moment correlation coefficient.

    Returns NaN when either series has zero variance — the metric is
    mathematically undefined there, and silently returning 0 would
    look like "no correlation" when the truth is "no signal in the
    inputs to detect". Callers should treat NaN as "insufficient
    signal" in the report.
    """
    if len(predictions) != len(labels):
        raise ValueError(
            f"length mismatch: {len(predictions)} predictions vs "
            f"{len(labels)} labels"
        )
    if len(predictions) < 2:
        raise ValueError(
            "pearson_correlation needs at least 2 paired samples"
        )
    pred_mean = statistics.fmean(predictions)
    label_mean = statistics.fmean(labels)
    pred_dev = [p - pred_mean for p in predictions]
    label_dev = [l - label_mean for l in labels]
    numerator = sum(pd * ld for pd, ld in zip(pred_dev, label_dev))
    pred_var = sum(pd * pd for pd in pred_dev)
    label_var = sum(ld * ld for ld in label_dev)
    if pred_var == 0.0 or label_var == 0.0:
        return float("nan")
    return numerator / math.sqrt(pred_var * label_var)


def top_k_agreement(
    predictions: Sequence[float], labels: Sequence[float], k: int
) -> float:
    """Fraction of the top-K labeled clips that appear in the top-K predicted.

    A ranking metric — independent of absolute score values, which
    makes it robust when the scorer is well-ordered but mis-scaled
    (e.g. tends to compress everything into [0.4, 0.7]).
    """
    if len(predictions) != len(labels):
        raise ValueError(
            f"length mismatch: {len(predictions)} predictions vs "
            f"{len(labels)} labels"
        )
    if k <= 0:
        raise ValueError(f"k must be positive, got {k}")
    n = len(predictions)
    if n == 0:
        return float("nan")
    effective_k = min(k, n)
    # Index-based ranking — argsort descending.
    pred_top = set(
        sorted(range(n), key=lambda i: predictions[i], reverse=True)[
            :effective_k
        ]
    )
    label_top = set(
        sorted(range(n), key=lambda i: labels[i], reverse=True)[
            :effective_k
        ]
    )
    overlap = len(pred_top & label_top)
    return overlap / effective_k


def calibration_bins(
    predictions: Sequence[float],
    labels: Sequence[float],
    n_bins: int = 10,
) -> list[dict[str, float | int]]:
    """Bin predictions into `n_bins` equal-width buckets across [0, 1].

    Returns one dict per bucket with the bucket's predicted-score
    range, population count, mean predicted score (i.e. the bin's
    horizontal coordinate on a reliability plot), and mean ground-
    truth label (the vertical coordinate). Empty buckets are reported
    with `count=0` and NaN means — keeping the bin axis dense makes
    the JSON report cheap to plot without bin-index alignment.
    """
    if len(predictions) != len(labels):
        raise ValueError(
            f"length mismatch: {len(predictions)} predictions vs "
            f"{len(labels)} labels"
        )
    if n_bins <= 0:
        raise ValueError(f"n_bins must be positive, got {n_bins}")
    bins: list[dict[str, float | int]] = []
    edge_lo = 0.0
    for i in range(n_bins):
        edge_hi = (i + 1) / n_bins
        # Right-closed on the final bin so prediction == 1.0 lands somewhere.
        members_p: list[float] = []
        members_l: list[float] = []
        for p, l in zip(predictions, labels):
            in_bin = edge_lo <= p < edge_hi if i < n_bins - 1 else edge_lo <= p <= edge_hi
            if in_bin:
                members_p.append(p)
                members_l.append(l)
        count = len(members_p)
        bins.append(
            {
                "pred_lo": edge_lo,
                "pred_hi": edge_hi,
                "count": count,
                "pred_mean": (
                    statistics.fmean(members_p) if count else float("nan")
                ),
                "label_mean": (
                    statistics.fmean(members_l) if count else float("nan")
                ),
            }
        )
        edge_lo = edge_hi
    return bins


# ---------------------------------------------------------------------------
# Per-fixture-set evaluation.
# ---------------------------------------------------------------------------


def _paired(
    predictions: Iterable[Mapping[str, float]],
    ground_truth: Iterable[Mapping[str, float]],
    kind: str,
) -> tuple[list[float], list[float]]:
    """Pull the (pred, label) pairs for one moment kind across clips.

    A clip contributes a pair only when *both* sides report the kind —
    a missing kind on either side is treated as "not labeled for this
    metric", never coerced to 0. (Coercing to 0 would penalise the
    scorer for clips the human rater didn't bother rating.)
    """
    paired_p: list[float] = []
    paired_l: list[float] = []
    for pred_dict, label_dict in zip(predictions, ground_truth):
        if kind in pred_dict and kind in label_dict:
            paired_p.append(float(pred_dict[kind]))
            paired_l.append(float(label_dict[kind]))
    return paired_p, paired_l


def evaluate(
    predictions: Sequence[Mapping[str, float]],
    ground_truth: Sequence[Mapping[str, float]],
    *,
    top_k: int = 5,
    n_bins: int = 10,
) -> dict[str, Any]:
    """Run all three metrics for every moment kind present in the data.

    `predictions[i]` and `ground_truth[i]` describe the same clip:
    both are `{kind: score}` dicts. Kinds present on one side but not
    the other are simply absent from that clip's contribution.
    """
    if len(predictions) != len(ground_truth):
        raise ValueError(
            f"length mismatch: {len(predictions)} prediction clips vs "
            f"{len(ground_truth)} ground-truth clips"
        )
    # Union of all kinds either side mentions — keeps "scorer-only"
    # and "human-only" tags visible in the report instead of swept
    # under the rug.
    kinds: set[str] = set()
    for d in predictions:
        kinds.update(d.keys())
    for d in ground_truth:
        kinds.update(d.keys())
    per_kind: dict[str, dict[str, Any]] = {}
    for kind in sorted(kinds):
        paired_p, paired_l = _paired(predictions, ground_truth, kind)
        n = len(paired_p)
        entry: dict[str, Any] = {
            "n": n,
            "top_k": top_k,
        }
        if n < 2:
            # Can't compute Pearson with <2 paired samples; flag it
            # rather than crash so a single-clip kind doesn't poison
            # the whole eval.
            entry["skipped"] = True
            entry["reason"] = (
                f"need >=2 paired observations to compute metrics; got {n}"
            )
            per_kind[kind] = entry
            continue
        entry["pearson"] = pearson_correlation(paired_p, paired_l)
        entry["top_k_agreement"] = top_k_agreement(
            paired_p, paired_l, top_k
        )
        entry["bins"] = calibration_bins(paired_p, paired_l, n_bins=n_bins)
        per_kind[kind] = entry
    return {
        "n_clips": len(predictions),
        "top_k": top_k,
        "n_bins": n_bins,
        "per_kind": per_kind,
    }


# ---------------------------------------------------------------------------
# Manifest + report I/O.
# ---------------------------------------------------------------------------


def load_manifest(manifest_path: Path) -> dict[str, Any]:
    """Load and minimally validate a fixture manifest.

    Expected shape:

        {
          "schema": "montage-editorial-moments-calibration/1",
          "clips": [
            { "clip_path": "...", "labels": {kind: 0..1}, "source": "..." }
          ]
        }

    We validate that every label is in [0, 1] — out-of-range values
    almost always indicate a unit-confusion bug (e.g. 0..100 percent
    leaked into the manifest) and silently propagating those would
    poison every metric.
    """
    doc = json.loads(Path(manifest_path).read_text())
    if "clips" not in doc or not isinstance(doc["clips"], list):
        raise ValueError(
            f"manifest at {manifest_path} missing 'clips' array"
        )
    for i, entry in enumerate(doc["clips"]):
        for key in ("clip_path", "labels", "source"):
            if key not in entry:
                raise ValueError(
                    f"manifest clip #{i} missing required field {key!r}"
                )
        for k, v in entry["labels"].items():
            if not (0.0 <= float(v) <= 1.0):
                raise ValueError(
                    f"manifest clip #{i} label {k}={v} out of [0, 1]"
                )
    return doc


def _fmt(value: Any) -> str:
    """Format a number for the markdown report; NaN renders as '—'."""
    if isinstance(value, float):
        if math.isnan(value):
            return "—"
        return f"{value:.3f}"
    return str(value)


def render_markdown(report: Mapping[str, Any]) -> str:
    """Render a calibration report as a human-readable markdown table.

    The intended reader is a developer triaging "is the scorer
    actually any good?" — so we lead with the headline per-kind table
    (Pearson, top-K agreement, n) and only render bin tables when a
    kind has enough data to compute them.
    """
    lines: list[str] = []
    lines.append("# Editorial-moments calibration report")
    lines.append("")
    if "fixtures" in report:
        lines.append(f"**Fixtures**: `{report['fixtures']}`")
    lines.append(f"**Clips**: {report.get('n_clips', '?')}")
    lines.append(f"**top-K**: {report.get('top_k', '?')}")
    lines.append(f"**bins**: {report.get('n_bins', '?')}")
    lines.append("")
    lines.append("## Per-moment-type metrics")
    lines.append("")
    lines.append("| kind | n | Pearson | top-K agreement |")
    lines.append("|------|---|---------|------------------|")
    for kind, entry in report.get("per_kind", {}).items():
        if entry.get("skipped"):
            lines.append(
                f"| {kind} | {entry.get('n', 0)} | (skipped: {entry.get('reason', 'insufficient data')}) | — |"
            )
            continue
        lines.append(
            f"| {kind} | {entry['n']} | {_fmt(entry['pearson'])} | "
            f"{_fmt(entry['top_k_agreement'])} |"
        )
    lines.append("")
    # Per-kind reliability bins.
    for kind, entry in report.get("per_kind", {}).items():
        if entry.get("skipped") or "bins" not in entry:
            continue
        lines.append(f"### Reliability bins — {kind}")
        lines.append("")
        lines.append("| pred_lo | pred_hi | count | pred_mean | label_mean |")
        lines.append("|---------|---------|-------|-----------|------------|")
        for b in entry["bins"]:
            lines.append(
                f"| {_fmt(b['pred_lo'])} | {_fmt(b['pred_hi'])} | "
                f"{b['count']} | {_fmt(b['pred_mean'])} | "
                f"{_fmt(b['label_mean'])} |"
            )
        lines.append("")
    return "\n".join(lines)


def write_report(report: Mapping[str, Any], json_path: Path) -> None:
    """Write the JSON report and its markdown sibling.

    NaN values are JSON-illegal, so we replace them with `None` on the
    way out — readers should treat null as "insufficient data" the
    same way `pearson_correlation` returns NaN in-process.
    """
    json_path = Path(json_path)
    json_path.parent.mkdir(parents=True, exist_ok=True)
    json_path.write_text(
        json.dumps(_json_safe(report), indent=2, sort_keys=False)
    )
    md_path = json_path.with_suffix(".md")
    md_path.write_text(render_markdown(report))


def _json_safe(value: Any) -> Any:
    if isinstance(value, Mapping):
        return {k: _json_safe(v) for k, v in value.items()}
    if isinstance(value, list):
        return [_json_safe(v) for v in value]
    if isinstance(value, float) and math.isnan(value):
        return None
    return value


__all__ = [
    "pearson_correlation",
    "top_k_agreement",
    "calibration_bins",
    "evaluate",
    "load_manifest",
    "render_markdown",
    "write_report",
]
