#!/usr/bin/env python3
"""Verify a rendered video with ffprobe when available.

The script always exits 0 and reports status in JSON so agents can read
and repair failures instead of crashing the turn.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
from pathlib import Path

from cut_window_view import render_cut_window


def probe(path: Path) -> dict:
    ffprobe = shutil.which("ffprobe")
    if not ffprobe:
        return {"ok": False, "error": "ffprobe not found"}
    proc = subprocess.run(
        [
            ffprobe,
            "-v", "error",
            "-show_entries", "format=duration:stream=codec_type",
            "-of", "json",
            str(path),
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if proc.returncode != 0:
        return {"ok": False, "error": proc.stderr.strip()}
    return {"ok": True, "raw": json.loads(proc.stdout)}


def plan_cut_windows(
    file_path: Path,
    cut_points_s: list[float],
    report_dir: Path,
    *,
    window_s: float = 3.0,
) -> list[dict]:
    windows = []
    for index, center_s in enumerate(cut_points_s, start=1):
        output_path = report_dir / f"cut-{index:03d}-{center_s:.3f}s.png"
        windows.append({
            "center_s": center_s,
            "window_s": window_s,
            "file": str(output_path),
            "source": str(file_path),
            "status": "planned",
        })
    return windows


def generate_cut_windows(
    file_path: Path,
    windows: list[dict],
    *,
    has_audio: bool,
) -> list[str]:
    errors = []
    for window in windows:
        result = render_cut_window(
            input_path=file_path,
            output_path=Path(window["file"]),
            center_s=float(window["center_s"]),
            window_s=float(window["window_s"]),
            has_audio=has_audio,
        )
        if result["ok"]:
            window["status"] = "generated"
        else:
            window["status"] = "failed"
            window["error"] = result["error"]
            errors.append(f"cut window {window['center_s']:.3f}s: {result['error']}")
    return errors


def verify_render(
    path: Path,
    *,
    expected_duration_s: float | None = None,
    max_duration_s: float | None = None,
    cut_points_s: list[float] | None = None,
    cut_report_dir: Path | None = None,
    cut_window_s: float = 3.0,
    skip_cut_window_images: bool = False,
) -> dict:
    checks = {
        "exists": path.exists(),
        "duration_ok": None,
        "has_audio": None,
        "has_video": None,
        "cut_windows": None,
    }
    errors = []
    duration = None
    cut_windows = []
    if not path.exists():
        errors.append("file does not exist")
    else:
        result = probe(path)
        if not result["ok"]:
            errors.append(result["error"])
        else:
            raw = result["raw"]
            duration = float(raw.get("format", {}).get("duration", 0.0))
            streams = [s.get("codec_type") for s in raw.get("streams", [])]
            checks["has_audio"] = "audio" in streams
            checks["has_video"] = "video" in streams
            if max_duration_s is not None:
                checks["duration_ok"] = duration <= max_duration_s
            elif expected_duration_s is not None:
                checks["duration_ok"] = abs(duration - expected_duration_s) <= 2.0
            if checks["has_audio"] is False:
                errors.append("no audio stream")
            if checks["has_video"] is False:
                errors.append("no video stream")
            if checks["duration_ok"] is False:
                errors.append("duration outside expected bounds")
            if cut_points_s:
                report_dir = cut_report_dir or Path(path.with_suffix("").name + "-verify")
                cut_windows = plan_cut_windows(
                    path,
                    cut_points_s,
                    report_dir,
                    window_s=cut_window_s,
                )
                if skip_cut_window_images:
                    checks["cut_windows"] = "planned"
                else:
                    window_errors = generate_cut_windows(
                        path,
                        cut_windows,
                        has_audio=bool(checks["has_audio"]),
                    )
                    checks["cut_windows"] = "generated" if not window_errors else "failed"
                    errors.extend(window_errors)

    return {
        "status": "passed" if not errors else "failed",
        "file": str(path),
        "duration_s": duration,
        "checks": checks,
        "cut_windows": cut_windows,
        "errors": errors,
    }


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--file", required=True)
    p.add_argument("--expected-duration-s", type=float)
    p.add_argument("--max-duration-s", type=float)
    p.add_argument("--cut-s", action="append", type=float, default=[])
    p.add_argument("--cut-report-dir")
    p.add_argument("--cut-window-s", type=float, default=3.0)
    p.add_argument("--skip-cut-window-images", action="store_true")
    args = p.parse_args()

    path = Path(args.file)
    print(json.dumps(verify_render(
        path,
        expected_duration_s=args.expected_duration_s,
        max_duration_s=args.max_duration_s,
        cut_points_s=args.cut_s,
        cut_report_dir=Path(args.cut_report_dir) if args.cut_report_dir else None,
        cut_window_s=args.cut_window_s,
        skip_cut_window_images=args.skip_cut_window_images,
    ), indent=2))


if __name__ == "__main__":
    main()
