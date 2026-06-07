#!/usr/bin/env python3
"""Run repeatable Python indexer smoke tiers.

Modes:
- safe: metadata/layout/schema only; no imports of heavy indexer modules.
- audio-energy: real MCP sidecar path through the Rust dispatcher against
  a tiny checked-in WAV fixture. No model downloads.
- full: explicit guard for human-run model indexer smoke. It prints the
  required commands and refuses to run unless MONTAGE_RUN_FULL_INDEXER_SMOKE=1
  is set, because full sync can download GBs and some indexers need gates.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

import smoke_safe


ROOT = Path(__file__).resolve().parents[2]
PYTHON_ROOT = ROOT / "python"


def run(cmd: list[str], *, cwd: Path = ROOT) -> None:
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, check=True)


def run_safe() -> None:
    smoke_safe.main()


def run_audio_energy() -> None:
    run(
        [
            "cargo",
            "test",
            "-p",
            "montage-index",
            "--test",
            "end_to_end",
            "audio_energy_indexer_writes_sidecar",
            "--",
            "--ignored",
            "--exact",
            "--nocapture",
        ]
    )


def run_full() -> None:
    if os.environ.get("MONTAGE_RUN_FULL_INDEXER_SMOKE") != "1":
        print(
            "full indexer smoke is intentionally guarded.\n"
            "It can download large model weights and may require HF_TOKEN and accepted model terms.\n\n"
            "To run it locally:\n"
            "  cd python && uv sync --all-packages\n"
            "  export MONTAGE_RUN_FULL_INDEXER_SMOKE=1\n"
            "  python scripts/smoke_indexers.py --full\n\n"
            "Then follow the real-asset commands in python/SMOKE.md for whisper/topic/clip/face/gaze/shot/frame-quality.",
            file=sys.stderr,
        )
        raise SystemExit(2)
    run(["uv", "sync", "--all-packages"], cwd=PYTHON_ROOT)
    print(
        "Full dependency sync completed. Run the real-asset smoke commands in python/SMOKE.md "
        "for model-backed indexers; this script does not fabricate large media or bypass HF gates."
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--safe", action="store_true", help="run metadata/schema smoke")
    parser.add_argument(
        "--audio-energy",
        action="store_true",
        help="run real audio-energy MCP sidecar smoke via Rust dispatcher",
    )
    parser.add_argument(
        "--full",
        action="store_true",
        help="guarded heavy dependency/model smoke setup",
    )
    args = parser.parse_args()

    if not (args.safe or args.audio_energy or args.full):
        args.safe = True

    if args.safe:
        run_safe()
    if args.audio_energy:
        run_audio_energy()
    if args.full:
        run_full()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
