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


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--file", required=True)
    p.add_argument("--expected-duration-s", type=float)
    p.add_argument("--max-duration-s", type=float)
    args = p.parse_args()

    path = Path(args.file)
    checks = {"exists": path.exists(), "duration_ok": None, "has_audio": None, "has_video": None}
    errors = []
    duration = None
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
            if args.max_duration_s is not None:
                checks["duration_ok"] = duration <= args.max_duration_s
            elif args.expected_duration_s is not None:
                checks["duration_ok"] = abs(duration - args.expected_duration_s) <= 2.0
            if checks["has_audio"] is False:
                errors.append("no audio stream")
            if checks["has_video"] is False:
                errors.append("no video stream")
            if checks["duration_ok"] is False:
                errors.append("duration outside expected bounds")
    print(json.dumps({
        "status": "passed" if not errors else "failed",
        "file": str(path),
        "duration_s": duration,
        "checks": checks,
        "errors": errors,
    }, indent=2))


if __name__ == "__main__":
    main()
