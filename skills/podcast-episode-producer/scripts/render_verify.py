#!/usr/bin/env python3
"""Verify a podcast render with ffprobe when available."""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
from pathlib import Path


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--file", required=True)
    p.add_argument("--min-duration-s", type=float, default=0.0)
    args = p.parse_args()
    path = Path(args.file)
    errors = []
    duration = None
    streams = []
    if not path.exists():
        errors.append("file does not exist")
    elif not shutil.which("ffprobe"):
        errors.append("ffprobe not found")
    else:
        proc = subprocess.run(
            ["ffprobe", "-v", "error", "-show_entries", "format=duration:stream=codec_type", "-of", "json", str(path)],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if proc.returncode != 0:
            errors.append(proc.stderr.strip())
        else:
            raw = json.loads(proc.stdout)
            duration = float(raw.get("format", {}).get("duration", 0.0))
            streams = [s.get("codec_type") for s in raw.get("streams", [])]
            if duration < args.min_duration_s:
                errors.append("duration below minimum")
            if "audio" not in streams:
                errors.append("no audio stream")
            if "video" not in streams:
                errors.append("no video stream")
    print(json.dumps({
        "status": "passed" if not errors else "failed",
        "file": str(path),
        "duration_s": duration,
        "streams": streams,
        "errors": errors,
    }, indent=2))


if __name__ == "__main__":
    main()
