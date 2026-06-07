#!/usr/bin/env python3
"""Download the default whisper.cpp model used by Montage's fast transcript path."""

from __future__ import annotations

import argparse
import hashlib
import os
import tempfile
import urllib.request
from pathlib import Path


DEFAULT_MODEL_NAME = "ggml-large-v3-turbo-q5_0.bin"
DEFAULT_URL = (
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/"
    "ggml-large-v3-turbo-q5_0.bin"
)
DEFAULT_SHA256 = "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2"
DEFAULT_DIR = Path.home() / ".cache" / "montage" / "whisper.cpp"


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def download(url: str, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(prefix=dest.name + ".", suffix=".tmp", dir=dest.parent)
    os.close(fd)
    tmp = Path(tmp_name)
    try:
        with urllib.request.urlopen(url) as response, tmp.open("wb") as out:
            while True:
                chunk = response.read(1024 * 1024)
                if not chunk:
                    break
                out.write(chunk)
        tmp.replace(dest)
    finally:
        if tmp.exists():
            tmp.unlink()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default=DEFAULT_URL)
    parser.add_argument("--sha256", default=DEFAULT_SHA256)
    parser.add_argument("--output-dir", type=Path, default=DEFAULT_DIR)
    parser.add_argument("--filename", default=DEFAULT_MODEL_NAME)
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()

    dest = args.output_dir.expanduser() / args.filename
    if dest.is_file() and not args.force:
        digest = sha256_file(dest)
        if digest == args.sha256:
            print(dest)
            return 0
        raise SystemExit(
            f"existing model checksum mismatch: {dest}\n"
            f"expected {args.sha256}\nactual   {digest}\n"
            "use --force to replace it"
        )

    download(args.url, dest)
    digest = sha256_file(dest)
    if digest != args.sha256:
        dest.unlink(missing_ok=True)
        raise SystemExit(
            f"download checksum mismatch: {dest}\n"
            f"expected {args.sha256}\nactual   {digest}"
        )
    print(dest)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
