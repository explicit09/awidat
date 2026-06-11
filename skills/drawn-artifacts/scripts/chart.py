#!/usr/bin/env python3
# /// script
# requires-python = ">=3.10"
# dependencies = ["matplotlib"]
# ///
"""Render brand-styled bar/line/comparison charts to a transparent PNG.

Run via uv (no system install needed):

    uv run --with matplotlib scripts/chart.py --spec spec.json --out chart.png
    uv run --with matplotlib scripts/chart.py --spec-json '{...}' --out chart.png

Spec schema (JSON):
    {
      "type": "bar" | "line" | "comparison",
      "title": "Optional title",
      "labels": ["Q1", "Q2", ...],
      "values": [10, 20, ...],            # bar/line
      "series": [                           # comparison (and multi-line)
        {"name": "2024", "values": [1, 2]},
        {"name": "2025", "values": [3, 4]}
      ],
      "y_label": "optional axis label",
      "panel": false                        # navy backing panel behind plot
    }
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

GOLD = "#C8A84E"
NAVY = "#070D17"
IVORY = "#F2EDE3"
GOLD_DIM = "#8A7436"
SERIES_COLORS = [GOLD, IVORY, GOLD_DIM, "#5B6B85"]


def _fail(msg: str) -> None:
    print(f"chart.py: {msg}", file=sys.stderr)
    sys.exit(1)


def load_spec(args: argparse.Namespace) -> dict:
    if args.spec_json:
        raw = args.spec_json
    elif args.spec:
        raw = Path(args.spec).read_text(encoding="utf-8")
    else:
        _fail("provide --spec <file.json> or --spec-json '<json>'")
    try:
        spec = json.loads(raw)
    except json.JSONDecodeError as exc:
        _fail(f"invalid JSON spec: {exc}")
    if not isinstance(spec, dict):
        _fail("spec must be a JSON object")
    return spec


def get_series(spec: dict) -> list[dict]:
    """Normalize bar/line `values` and comparison `series` to one shape."""
    if "series" in spec:
        series = spec["series"]
        if not isinstance(series, list) or not series:
            _fail("`series` must be a non-empty array of {name, values}")
        return series
    if "values" in spec:
        return [{"name": spec.get("title", ""), "values": spec["values"]}]
    _fail("spec needs `values` (bar/line) or `series` (comparison)")
    raise AssertionError  # unreachable


def style_axes(ax, spec: dict) -> None:
    ax.set_facecolor("none")
    for spine in ("top", "right"):
        ax.spines[spine].set_visible(False)
    for spine in ("left", "bottom"):
        ax.spines[spine].set_color(IVORY)
        ax.spines[spine].set_linewidth(1.2)
    ax.tick_params(colors=IVORY, labelsize=14)
    ax.grid(axis="y", color=IVORY, alpha=0.15, linewidth=0.8)
    ax.set_axisbelow(True)
    if spec.get("y_label"):
        ax.set_ylabel(spec["y_label"], color=IVORY, fontsize=15)
    if spec.get("title"):
        ax.set_title(
            spec["title"], color=GOLD, fontsize=22, fontweight="bold", pad=18
        )


def draw(spec: dict, out_path: Path, width: int, height: int, dpi: int) -> None:
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    import numpy as np

    chart_type = spec.get("type", "bar")
    labels = spec.get("labels")
    series = get_series(spec)
    if not labels:
        _fail("spec needs `labels`")
    for s in series:
        if len(s.get("values", [])) != len(labels):
            _fail(f"series {s.get('name')!r}: values length != labels length")

    fig, ax = plt.subplots(figsize=(width / dpi, height / dpi), dpi=dpi)
    fig.patch.set_alpha(0.0)
    if spec.get("panel"):
        fig.patch.set_facecolor(NAVY)
        fig.patch.set_alpha(0.85)

    x = np.arange(len(labels))
    if chart_type == "bar":
        ax.bar(x, series[0]["values"], width=0.62, color=GOLD, zorder=3)
    elif chart_type == "line":
        for i, s in enumerate(series):
            color = SERIES_COLORS[i % len(SERIES_COLORS)]
            ax.plot(x, s["values"], color=color, linewidth=3.0, marker="o",
                    markersize=7, label=s.get("name") or None, zorder=3)
    elif chart_type == "comparison":
        n = len(series)
        bar_w = 0.8 / n
        for i, s in enumerate(series):
            color = SERIES_COLORS[i % len(SERIES_COLORS)]
            ax.bar(x + (i - (n - 1) / 2) * bar_w, s["values"], width=bar_w,
                   color=color, label=s.get("name") or None, zorder=3)
    else:
        _fail(f"unknown chart type {chart_type!r} (bar|line|comparison)")

    ax.set_xticks(x)
    ax.set_xticklabels(labels)
    style_axes(ax, spec)
    if len(series) > 1 or (chart_type == "line" and series[0].get("name")):
        legend = ax.legend(frameon=False, fontsize=13)
        for text in legend.get_texts():
            text.set_color(IVORY)

    fig.tight_layout()
    out_path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out_path, transparent=not spec.get("panel"), dpi=dpi)
    plt.close(fig)
    print(json.dumps({"status": "ok", "output": str(out_path),
                      "width_px": width, "height_px": height}))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--spec", help="path to JSON spec file")
    parser.add_argument("--spec-json", help="inline JSON spec string")
    parser.add_argument("--out", required=True, help="output PNG path")
    parser.add_argument("--width", type=int, default=1600, help="px (default 1600)")
    parser.add_argument("--height", type=int, default=900, help="px (default 900)")
    parser.add_argument("--dpi", type=int, default=200)
    args = parser.parse_args()
    draw(load_spec(args), Path(args.out), args.width, args.height, args.dpi)


if __name__ == "__main__":
    main()
