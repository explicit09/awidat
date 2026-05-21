#!/usr/bin/env python3
"""Editorial-moments calibration harness CLI.

Run the editorial-moments scorer against a manifest of human-labeled
clips, then emit a JSON + markdown report measuring how well the
predicted per-moment-type scores correlate with the human labels.

Typical flow
------------
    python scripts/calibrate.py \
        --fixtures fixtures/calibration/manifest.json \
        --out reports/calibration.json \
        --top-k 5 \
        --n-bins 10

The CLI is intentionally model-agnostic for the predictions side: it
accepts a `--predictions` JSON path that mirrors the manifest's clip
order. The actual scorer invocation lives in the MCP server entry
point and may be slow / require ANTHROPIC_API_KEY, so we keep the
"compute metrics from labels + predictions" step independent of the
"run the scorer" step. That separation is what lets the unit tests
exercise the harness without touching the LLM.

If `--predictions` is omitted, we look for `<manifest>.predictions.json`
next to the manifest. If that's missing too, we exit with a clear
message pointing at the script that would generate it.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

# Allow `python scripts/calibrate.py` from the package root without
# `uv run` setting up sys.path for us.
PKG_SRC = Path(__file__).resolve().parents[1] / "src"
if str(PKG_SRC) not in sys.path:
    sys.path.insert(0, str(PKG_SRC))

from editorial_moments_mcp import calibration  # noqa: E402


def _load_predictions(path: Path) -> list[dict[str, float]]:
    """Load the per-clip predictions sidecar.

    Expected shape — a list aligned with the manifest's `clips`:

        [
          {"hook": 0.9, "punchline": 0.1, ...},
          {"hook": 0.1, "punchline": 0.85, ...},
          ...
        ]

    We deliberately don't reuse the EditorialMoment schema here —
    that schema is per-beat, this is per-clip aggregate. The
    aggregation policy (e.g. "max score across beats of this kind")
    is the responsibility of whatever generated this file.
    """
    doc = json.loads(path.read_text())
    if not isinstance(doc, list):
        raise ValueError(
            f"predictions at {path} must be a list of {{kind: score}} dicts; "
            f"got {type(doc).__name__}"
        )
    return doc


def _parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Compute editorial-moments calibration metrics (Pearson, "
            "top-K agreement, calibration bins) over a labeled-clip "
            "manifest."
        )
    )
    parser.add_argument(
        "--fixtures",
        required=True,
        type=Path,
        help="Path to manifest.json (clips + ground-truth labels).",
    )
    parser.add_argument(
        "--predictions",
        type=Path,
        default=None,
        help=(
            "Path to predictions.json (per-clip {kind: score} dicts in "
            "manifest order). Defaults to <fixtures>.predictions.json."
        ),
    )
    parser.add_argument(
        "--out",
        required=True,
        type=Path,
        help="Path to write the JSON report (markdown sibling .md is also written).",
    )
    parser.add_argument(
        "--top-k",
        type=int,
        default=5,
        help="Top-K agreement cutoff (default: 5).",
    )
    parser.add_argument(
        "--n-bins",
        type=int,
        default=10,
        help="Number of equal-width calibration bins across [0, 1].",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = _parse_args(argv)

    manifest = calibration.load_manifest(args.fixtures)
    clips = manifest["clips"]
    ground_truth = [c["labels"] for c in clips]

    predictions_path = args.predictions
    if predictions_path is None:
        predictions_path = args.fixtures.with_suffix(
            args.fixtures.suffix + ".predictions.json"
        )
    if not predictions_path.exists():
        # Fail loud and explicit — silently substituting zeros would
        # poison every metric (per the CORE PRINCIPLES "Fail Fast"
        # rule).
        print(
            f"calibrate.py: no predictions file at {predictions_path}.\n"
            "Run the editorial-moments-mcp scorer over the manifest "
            "clips and write a per-clip {kind: score} JSON list to "
            "that path, then re-run this script.",
            file=sys.stderr,
        )
        return 2
    predictions = _load_predictions(predictions_path)

    if len(predictions) != len(clips):
        print(
            f"calibrate.py: predictions count ({len(predictions)}) does "
            f"not match manifest clip count ({len(clips)}). Predictions "
            "must be aligned by index.",
            file=sys.stderr,
        )
        return 2

    report = calibration.evaluate(
        predictions=predictions,
        ground_truth=ground_truth,
        top_k=args.top_k,
        n_bins=args.n_bins,
    )
    report["fixtures"] = str(args.fixtures)
    report["predictions"] = str(predictions_path)
    calibration.write_report(report, args.out)

    md_path = args.out.with_suffix(".md")
    print(
        f"calibrate.py: wrote {args.out} and {md_path} "
        f"(clips={len(clips)}, kinds={len(report['per_kind'])})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
