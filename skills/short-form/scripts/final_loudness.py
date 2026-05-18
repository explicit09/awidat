#!/usr/bin/env python3
"""Apply measured final loudness normalization to a rendered file."""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
from pathlib import Path


REQUIRED_MEASUREMENT_KEYS = {
    "input_i",
    "input_tp",
    "input_lra",
    "input_thresh",
    "target_offset",
}


def parse_loudnorm_measurement(stderr: str) -> dict:
    matches = re.findall(r"\{[\s\S]*?\}", stderr)
    for candidate in reversed(matches):
        try:
            data = json.loads(candidate)
        except json.JSONDecodeError:
            continue
        if REQUIRED_MEASUREMENT_KEYS.issubset(data):
            return {key: str(data[key]) for key in REQUIRED_MEASUREMENT_KEYS}
    raise ValueError("ffmpeg loudnorm measurement JSON not found")


def build_first_pass_args(
    *,
    ffmpeg: str,
    input_path: Path,
    target_i: float,
    target_tp: float,
    target_lra: float,
) -> list[str]:
    return [
        ffmpeg,
        "-hide_banner",
        "-nostats",
        "-i",
        str(input_path),
        "-map",
        "0:a:0",
        "-af",
        f"loudnorm=I={target_i}:TP={target_tp}:LRA={target_lra}:print_format=json",
        "-f",
        "null",
        "-",
    ]


def build_second_pass_args(
    *,
    ffmpeg: str,
    input_path: Path,
    output_path: Path,
    measurement: dict,
    target_i: float,
    target_tp: float,
    target_lra: float,
) -> list[str]:
    audio_filter = (
        f"loudnorm=I={target_i}:TP={target_tp}:LRA={target_lra}:"
        f"measured_I={measurement['input_i']}:"
        f"measured_TP={measurement['input_tp']}:"
        f"measured_LRA={measurement['input_lra']}:"
        f"measured_thresh={measurement['input_thresh']}:"
        f"offset={measurement['target_offset']}:linear=true:print_format=summary"
    )
    return [
        ffmpeg,
        "-y",
        "-hide_banner",
        "-i",
        str(input_path),
        "-map",
        "0:v?",
        "-map",
        "0:a:0",
        "-c:v",
        "copy",
        "-af",
        audio_filter,
        "-c:a",
        "aac",
        "-b:a",
        "192k",
        str(output_path),
    ]


def run_final_loudness(
    *,
    input_path: Path,
    output_path: Path,
    target_i: float,
    target_tp: float,
    target_lra: float,
) -> dict:
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        return {"status": "failed", "error": "ffmpeg not found"}
    first = subprocess.run(
        build_first_pass_args(
            ffmpeg=ffmpeg,
            input_path=input_path,
            target_i=target_i,
            target_tp=target_tp,
            target_lra=target_lra,
        ),
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if first.returncode != 0:
        return {"status": "failed", "error": first.stderr.strip()}
    try:
        measurement = parse_loudnorm_measurement(first.stderr)
    except ValueError as exc:
        return {"status": "failed", "error": str(exc)}

    output_path.parent.mkdir(parents=True, exist_ok=True)
    second = subprocess.run(
        build_second_pass_args(
            ffmpeg=ffmpeg,
            input_path=input_path,
            output_path=output_path,
            measurement=measurement,
            target_i=target_i,
            target_tp=target_tp,
            target_lra=target_lra,
        ),
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if second.returncode != 0:
        return {"status": "failed", "error": second.stderr.strip(), "measurement": measurement}
    return {
        "status": "passed",
        "input": str(input_path),
        "output": str(output_path),
        "target_i": target_i,
        "target_tp": target_tp,
        "target_lra": target_lra,
        "measurement": measurement,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--file", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--target-i", type=float, default=-14.0)
    parser.add_argument("--target-tp", type=float, default=-1.0)
    parser.add_argument("--target-lra", type=float, default=11.0)
    args = parser.parse_args()

    print(json.dumps(run_final_loudness(
        input_path=Path(args.file),
        output_path=Path(args.out),
        target_i=args.target_i,
        target_tp=args.target_tp,
        target_lra=args.target_lra,
    ), indent=2))


if __name__ == "__main__":
    main()
