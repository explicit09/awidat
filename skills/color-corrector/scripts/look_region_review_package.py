#!/usr/bin/env python3
"""Create review artifacts for a rendered look-region/LUT pass.

This script closes the loop after look_region_plan.py:

1. Read the look-region plan JSON.
2. Sample the rendered timeline at the plan's timeline sample times.
3. Generate a contact sheet from the actual render output.
4. Write JSON and Markdown review packages summarizing regions, LUTs,
   consistency groups, and manual-review risks.

It does not edit the project graph. The EDL/application/render steps
remain separate and auditable.
"""

from __future__ import annotations

import argparse
import json
import math
import subprocess
from pathlib import Path
from typing import Any


METRIC_WIDTH = 160
METRIC_HEIGHT = 90


def load_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def as_float(value: Any) -> float | None:
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def region_samples(region: dict[str, Any]) -> list[float]:
    raw = region.get("sample_times_s")
    if isinstance(raw, list):
        values = [as_float(v) for v in raw]
        samples = [round(v, 3) for v in values if v is not None and v >= 0.0]
        if samples:
            return samples

    start = as_float(region.get("timeline_start_s"))
    end = as_float(region.get("timeline_end_s"))
    if start is None or end is None or end <= start:
        return [0.0]
    mid = start + ((end - start) / 2.0)
    return sorted({round(start + 0.1, 3), round(mid, 3), round(max(start, end - 0.1), 3)})


def sample_manifest(regions: list[dict[str, Any]], max_samples: int) -> tuple[list[dict[str, Any]], list[float]]:
    entries: list[dict[str, Any]] = []
    seen: set[float] = set()
    for idx, region in enumerate(regions):
        for t_s in region_samples(region):
            if t_s in seen:
                continue
            seen.add(t_s)
            entries.append(
                {
                    "region_index": idx,
                    "clip_name": region.get("clip_name"),
                    "scene_id": region.get("scene_id"),
                    "timeline_s": t_s,
                    "look_id": region.get("look_id"),
                    "lut_path": region.get("lut_path"),
                }
            )

    entries.sort(key=lambda item: item["timeline_s"])
    if max_samples > 0 and len(entries) > max_samples:
        if max_samples == 1:
            entries = [entries[len(entries) // 2]]
        else:
            last = len(entries) - 1
            chosen: list[dict[str, Any]] = []
            used: set[int] = set()
            for slot in range(max_samples):
                idx = round(slot * last / (max_samples - 1))
                if idx not in used:
                    used.add(idx)
                    chosen.append(entries[idx])
            entries = chosen
    return entries, [entry["timeline_s"] for entry in entries]


def run_contact_sheet(
    script_dir: Path,
    after_render: Path,
    output: Path,
    times: list[float],
    tile_width: int,
    before_render: Path | None,
) -> dict[str, Any]:
    args = [
        "python3",
        str(script_dir / "rendered_contact_sheet.py"),
        "--after-render",
        str(after_render),
        "--output",
        str(output),
        "--times",
        ",".join(f"{t_s:.3f}" for t_s in times),
        "--tile-width",
        str(tile_width),
    ]
    if before_render:
        args.extend(["--before-render", str(before_render)])
    proc = subprocess.run(
        args,
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


def frame_metrics(path: Path, t_s: float) -> dict[str, float]:
    proc = subprocess.run(
        [
            "ffmpeg",
            "-nostdin",
            "-v",
            "error",
            "-ss",
            f"{t_s:.3f}",
            "-i",
            str(path),
            "-frames:v",
            "1",
            "-vf",
            f"scale={METRIC_WIDTH}:{METRIC_HEIGHT}",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "-",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    expected = METRIC_WIDTH * METRIC_HEIGHT * 3
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.decode("utf-8", errors="replace").strip())
    if len(proc.stdout) != expected:
        raise RuntimeError(f"expected {expected} RGB bytes, got {len(proc.stdout)}")

    luma = []
    for idx in range(0, len(proc.stdout), 3):
        r = proc.stdout[idx] / 255.0
        g = proc.stdout[idx + 1] / 255.0
        b = proc.stdout[idx + 2] / 255.0
        luma.append((0.2126 * r) + (0.7152 * g) + (0.0722 * b))

    mean = sum(luma) / len(luma)
    variance = sum((value - mean) ** 2 for value in luma) / len(luma)
    ordered = sorted(luma)
    return {
        "mean_luma": round(mean, 4),
        "contrast_stdev": round(math.sqrt(variance), 4),
        "p05_luma": round(ordered[int(0.05 * (len(ordered) - 1))], 4),
        "p95_luma": round(ordered[int(0.95 * (len(ordered) - 1))], 4),
        "clipped_dark_fraction": round(sum(1 for value in luma if value <= 0.02) / len(luma), 4),
        "clipped_bright_fraction": round(sum(1 for value in luma if value >= 0.98) / len(luma), 4),
    }


def visual_quality_review(
    samples: list[dict[str, Any]],
    after_render: Path,
    before_render: Path | None,
) -> dict[str, Any]:
    metrics = []
    flags = []
    errors = []
    for sample in samples:
        t_s = as_float(sample.get("timeline_s"))
        if t_s is None:
            continue
        try:
            after = frame_metrics(after_render, t_s)
            before = frame_metrics(before_render, t_s) if before_render else None
        except Exception as exc:  # noqa: BLE001 - captured for agent repair loops.
            errors.append({"timeline_s": t_s, "error": str(exc)})
            continue

        entry = {
            "region_index": sample.get("region_index"),
            "timeline_s": t_s,
            "look_id": sample.get("look_id"),
            "after": after,
        }
        if before:
            entry["before"] = before
        metrics.append(entry)

        after_mean = after["mean_luma"]
        after_std = after["contrast_stdev"]
        after_p05 = after["p05_luma"]
        if after_mean >= 0.50 and after_std <= 0.09 and after_p05 >= 0.34:
            flags.append(
                {
                    "region_index": sample.get("region_index"),
                    "timeline_s": t_s,
                    "issue": "washed_out",
                    "severity": "high",
                    "after": after,
                    "recommendation": "Reduce exposure/lift for this region before accepting the LUT pass.",
                }
            )

        if before:
            before_mean = before["mean_luma"]
            before_std = before["contrast_stdev"]
            before_p05 = before["p05_luma"]
            mean_drift = round(after_mean - before_mean, 4)
            contrast_ratio = round(after_std / before_std, 4) if before_std > 0.0 else None
            if mean_drift >= 0.22 and after_mean >= 0.35 and (contrast_ratio is None or contrast_ratio <= 0.95 or after_p05 >= before_p05 + 0.20):
                flags.append(
                    {
                        "region_index": sample.get("region_index"),
                        "timeline_s": t_s,
                        "issue": "excessive_lift",
                        "severity": "high",
                        "mean_luma_drift": mean_drift,
                        "contrast_ratio": contrast_ratio,
                        "before": before,
                        "after": after,
                        "recommendation": "Back off exposure/shadow lift or choose a darker LUT candidate for this region.",
                    }
                )
            if before_std >= 0.08 and contrast_ratio is not None and contrast_ratio <= 0.65 and mean_drift >= 0.12:
                flags.append(
                    {
                        "region_index": sample.get("region_index"),
                        "timeline_s": t_s,
                        "issue": "contrast_collapse",
                        "severity": "medium",
                        "mean_luma_drift": mean_drift,
                        "contrast_ratio": contrast_ratio,
                        "before": before,
                        "after": after,
                        "recommendation": "Preserve local contrast when lifting exposure.",
                    }
                )
            if mean_drift <= -0.18 and after_mean <= 0.08:
                flags.append(
                    {
                        "region_index": sample.get("region_index"),
                        "timeline_s": t_s,
                        "issue": "excessive_darkening",
                        "severity": "medium",
                        "mean_luma_drift": mean_drift,
                        "before": before,
                        "after": after,
                        "recommendation": "Reduce contrast or darkening introduced by the look.",
                    }
                )

    return {
        "status": "passed" if not flags and not errors else "needs_review",
        "metrics": metrics,
        "flags": flags,
        "errors": errors,
    }


def policy_confidence(region: dict[str, Any]) -> float | None:
    policy = region.get("policy")
    if isinstance(policy, dict):
        value = as_float(policy.get("confidence"))
        if value is not None:
            return value
    return as_float(region.get("score"))


def summarize_regions(regions: list[dict[str, Any]], low_confidence: float, visual: dict[str, Any]) -> dict[str, Any]:
    review_only = []
    low_conf = []
    groups: dict[str, list[int]] = {}
    generated_luts: set[str] = set()

    for idx, region in enumerate(regions):
        policy = region.get("policy") if isinstance(region.get("policy"), dict) else {}
        if policy.get("recommended_action") == "review_only":
            review_only.append(idx)
        confidence = policy_confidence(region)
        if confidence is not None and confidence < low_confidence:
            low_conf.append(idx)
        group = region.get("consistency_group")
        if isinstance(group, str) and group:
            groups.setdefault(group, []).append(idx)
        lut_path = region.get("lut_path")
        if isinstance(lut_path, str) and lut_path:
            generated_luts.add(lut_path)

    repeated_groups = {key: value for key, value in groups.items() if len(value) > 1}
    visual_flags = visual.get("flags") if isinstance(visual.get("flags"), list) else []
    visual_regions = sorted({flag.get("region_index") for flag in visual_flags if flag.get("region_index") is not None})
    return {
        "region_count": len(regions),
        "lut_region_count": sum(1 for region in regions if region.get("lut_path")),
        "generated_luts": sorted(generated_luts),
        "review_only_regions": review_only,
        "low_confidence_regions": low_conf,
        "consistency_groups": groups,
        "repeated_consistency_groups": repeated_groups,
        "visual_quality_status": visual.get("status", "unknown"),
        "visual_quality_flag_count": len(visual_flags),
        "visual_quality_flagged_regions": visual_regions,
    }


def status_for(contact: dict[str, Any], regions: list[dict[str, Any]], visual: dict[str, Any]) -> str:
    if not regions:
        return "failed"
    if contact.get("status") != "passed":
        return "failed"
    if visual.get("errors"):
        return "failed"
    if visual.get("flags"):
        return "needs_review"
    return "passed"


def write_report(path: Path, package: dict[str, Any]) -> None:
    summary = package["summary"]
    lines = [
        "# Look Region Review Package",
        "",
        f"- Status: `{package['status']}`",
        f"- Plan: `{package['look_plan']}`",
        f"- Render: `{package['after_render']}`",
        f"- Before render: `{package.get('before_render') or 'not provided'}`",
        f"- Contact sheet: `{package.get('contact_sheet_path') or 'not generated'}`",
        f"- Regions: {summary['region_count']}",
        f"- LUT regions: {summary['lut_region_count']}",
        f"- Sampled frames: {len(package['samples'])}",
        "",
        "## Generated LUTs",
        "",
    ]
    if summary["generated_luts"]:
        lines.extend([f"- `{path}`" for path in summary["generated_luts"]])
    else:
        lines.append("- None.")

    lines.extend(["", "## Manual Review Flags", ""])
    if summary["review_only_regions"]:
        lines.append(f"- Review-only regions: {summary['review_only_regions']}")
    else:
        lines.append("- Review-only regions: none")
    if summary["low_confidence_regions"]:
        lines.append(f"- Low-confidence regions: {summary['low_confidence_regions']}")
    else:
        lines.append("- Low-confidence regions: none")
    if summary["repeated_consistency_groups"]:
        lines.append(f"- Repeated consistency groups: {len(summary['repeated_consistency_groups'])}")
    else:
        lines.append("- Repeated consistency groups: none")
    if summary["visual_quality_flag_count"]:
        lines.append(f"- Visual quality flags: {summary['visual_quality_flag_count']} across regions {summary['visual_quality_flagged_regions']}")
    else:
        lines.append("- Visual quality flags: none")

    lines.extend(["", "## Visual Quality Gate", ""])
    visual = package.get("visual_quality") if isinstance(package.get("visual_quality"), dict) else {}
    lines.append(f"- Status: `{visual.get('status', 'unknown')}`")
    if visual.get("flags"):
        for flag in visual["flags"]:
            lines.append(
                f"- Region {flag.get('region_index')} at {float(flag.get('timeline_s', 0.0)):.3f}s: "
                f"`{flag.get('issue')}` ({flag.get('severity')}) - {flag.get('recommendation')}"
            )
    else:
        lines.append("- No visual quality flags.")

    lines.extend(["", "## Regions", ""])
    for idx, region in enumerate(package["regions"]):
        samples = ", ".join(f"{t_s:.3f}s" for t_s in region.get("sample_times_s", []))
        lines.extend(
            [
                f"### Region {idx}: {region.get('clip_name') or 'unknown'} / {region.get('scene_id') or 'scene'}",
                "",
                f"- Timeline: {float(region.get('timeline_start_s', 0.0)):.3f}s to {float(region.get('timeline_end_s', 0.0)):.3f}s",
                f"- Look: `{region.get('look_id') or 'none'}`",
                f"- LUT: `{region.get('lut_path') or 'none'}`",
                f"- Consistency group: `{region.get('consistency_group') or 'none'}`",
                f"- Samples: {samples or 'none'}",
                f"- Tags: {', '.join(region.get('issue_tags') or []) or 'none'}",
                "",
            ]
        )

    if package["contact_sheet"].get("errors"):
        lines.extend(["## Contact Sheet Errors", ""])
        lines.extend([f"- {err}" for err in package["contact_sheet"]["errors"] if err])

    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--look-plan", required=True)
    parser.add_argument("--after-render", required=True)
    parser.add_argument("--before-render")
    parser.add_argument("--contact-sheet", required=True)
    parser.add_argument("--report-md", required=True)
    parser.add_argument("--package-json")
    parser.add_argument("--max-samples", type=int, default=24)
    parser.add_argument("--tile-width", type=int, default=320)
    parser.add_argument("--low-confidence", type=float, default=0.5)
    args = parser.parse_args()

    plan = load_json(Path(args.look_plan))
    regions = list(plan.get("regions") or [])
    samples, times = sample_manifest(regions, args.max_samples)
    if not times:
        times = [0.0]
    before_render = Path(args.before_render) if args.before_render else None
    contact = run_contact_sheet(
        Path(__file__).resolve().parent,
        Path(args.after_render),
        Path(args.contact_sheet),
        times,
        args.tile_width,
        before_render,
    )
    visual = visual_quality_review(samples, Path(args.after_render), before_render)
    summary = summarize_regions(regions, args.low_confidence, visual)
    package = {
        "version": 1,
        "status": status_for(contact, regions, visual),
        "look_plan": args.look_plan,
        "after_render": args.after_render,
        "before_render": args.before_render,
        "contact_sheet_path": args.contact_sheet if contact.get("status") == "passed" else None,
        "contact_sheet": contact,
        "visual_quality": visual,
        "samples": samples,
        "summary": summary,
        "regions": regions,
        "report_md": args.report_md,
    }
    write_report(Path(args.report_md), package)
    if args.package_json:
        Path(args.package_json).parent.mkdir(parents=True, exist_ok=True)
        Path(args.package_json).write_text(json.dumps(package, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(package, indent=2))


if __name__ == "__main__":
    main()
