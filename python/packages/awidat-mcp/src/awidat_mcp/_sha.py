"""Streaming SHA-256, identical to the Rust impl in `crates/index/src/sha.rs`.

Reads the file in 64 KiB chunks. Memory-bounded; safe on multi-GB videos.
"""

from __future__ import annotations

import hashlib
from pathlib import Path


def asset_sha256(path: str | Path) -> str:
    """Return the lowercase-hex SHA-256 of the file at `path`."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(64 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()
