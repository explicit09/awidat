#!/usr/bin/env python3
"""Create a JSON/Markdown color review package from rendered artifacts."""

from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path
from typing import Any


def load_body(path: str) -> dict[str, Any]:
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def run_contact_sheet(script_dir: Path, before: str, after: str, output: str, times: str) -> dict[str, Any]:
    proc = subprocess.run(
        [
            "python3",
            str(script_dir / "rendered_contact_sheet.py"),
            "--before-render",
            before,
            "--after-render",
            after,
            "--output",
            output,
            "--times",
            times,
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    try:
        body = json.loads(proc.stdout)
    except json.JSONDecodeError:
        body = {"status": "failed", "contact_sheet": None, "errors": [proc.stderr.strip() or proc.stdout.strip()]}
    if proc.returncode != 0 and body.get("status") != "failed":
        body["status"] = "failed"
        body.setdefault("errors", []).append(proc.stderr.strip())
    return body


def write_report(path: Path, package: dict[str, Any]) -> None:
    summary = package["color_summary"]
    policy = summary.get("policy", {})
    correction = summary.get("recommended_correction", {})
    lines = [
        "# Color Review Package",
        "",
        f"- Policy action: `{policy.get('recommended_action', 'unknown')}`",
        f"- Issue tags: {', '.join(summary.get('issue_tags', [])) or 'none'}",
        f"- Confidence: {summary.get('confidence', 0.0):.3f}",
        f"- Before render: `{package['before_render']}`",
        f"- After render: `{package['after_render']}`",
        f"- Contact sheet: `{package.get('contact_sheet_path') or 'not generated'}`",
        "",
        "## Proposed Correction",
        "",
    ]
    if correction:
        lines.extend([f"- `{k}`: {v}" for k, v in correction.items()])
    else:
        lines.append("- No correction proposed.")
    lines.extend(["", "## Residual Risk", ""])
    lines.append(package["residual_risk"])
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--color-index", required=True)
    parser.add_argument("--before-render", required=True)
    parser.add_argument("--after-render", required=True)
    parser.add_argument("--contact-sheet", required=True)
    parser.add_argument("--report-md", required=True)
    parser.add_argument("--benchmark-json")
    parser.add_argument("--times", default="0,5,10")
    args = parser.parse_args()

    data = load_body(args.color_index)
    summary = data.get("summary", {})
    policy = summary.get("policy", {})
    contact = run_contact_sheet(
        Path(__file__).resolve().parent,
        args.before_render,
        args.after_render,
        args.contact_sheet,
        args.times,
    )
    residual = "Review required before applying automatically." if policy.get("requires_review") else "No major residual risk flagged by color policy."
    if "unsafe_to_auto_correct" in summary.get("issue_tags", []):
        residual = "Unsafe auto-correction signal present; inspect frames before applying graph edits."

    package = {
        "status": "passed" if contact.get("status") == "passed" else "failed",
        "before_render": args.before_render,
        "after_render": args.after_render,
        "contact_sheet_path": args.contact_sheet if contact.get("status") == "passed" else None,
        "contact_sheet": contact,
        "color_summary": summary,
        "residual_risk": residual,
        "report_md": args.report_md,
    }
    write_report(Path(args.report_md), package)
    if args.benchmark_json:
        Path(args.benchmark_json).parent.mkdir(parents=True, exist_ok=True)
        Path(args.benchmark_json).write_text(json.dumps(package, indent=2))
    print(json.dumps(package, indent=2))


if __name__ == "__main__":
    main()
