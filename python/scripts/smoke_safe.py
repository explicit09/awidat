#!/usr/bin/env python3
"""Safe Python indexer smoke checks.

This script is intentionally metadata/schema-only. It does not import
heavy indexer modules, run ffmpeg, download models, or contact gated
Hugging Face resources. Use python/SMOKE.md for the full manual smoke.
"""

from __future__ import annotations

import json
import sys
import tomllib
from datetime import datetime, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PACKAGES = ROOT / "packages"
REQUIRED_SIDECAR_KEYS = {
    "indexer",
    "indexer_version",
    "schema_version",
    "asset_id",
    "asset_sha256",
    "produced_at",
    "data",
}


def fail(message: str) -> None:
    raise SystemExit(f"python smoke failed: {message}")


def load_toml(path: Path) -> dict:
    with path.open("rb") as f:
        return tomllib.load(f)


def check_workspace_members() -> list[str]:
    root = load_toml(ROOT / "pyproject.toml")
    members = root["tool"]["uv"]["workspace"]["members"]
    package_names: list[str] = []
    for member in members:
        path = ROOT / member / "pyproject.toml"
        if not path.is_file():
            fail(f"workspace member {member!r} has no pyproject.toml")
        project = load_toml(path)["project"]
        package_names.append(project["name"])
    return package_names


def module_name_for(package_name: str) -> str:
    return package_name.replace("-", "_")


def check_package_layout(package_names: list[str]) -> None:
    for package_name in package_names:
        module = module_name_for(package_name)
        init = PACKAGES / package_name / "src" / module / "__init__.py"
        if not init.is_file():
            fail(f"{package_name} missing src/{module}/__init__.py")
        text = init.read_text()
        if package_name == "awidat-mcp":
            for exported in ["Sidecar", "IndexerServer", "IndexAssetRequest"]:
                if exported not in text:
                    fail(f"awidat-mcp __init__.py does not export {exported}")
        else:
            for marker in ["INDEXER_NAME", "INDEXER_VERSION", "SCHEMA_VERSION"]:
                if marker not in text:
                    fail(f"{package_name} missing {marker} marker")


def check_sidecar_schema() -> None:
    sidecar = {
        "indexer": "smoke",
        "indexer_version": "0.0.0",
        "schema_version": "1",
        "asset_id": "raw/synthetic.wav",
        "asset_sha256": "0" * 64,
        "produced_at": datetime.now(timezone.utc).isoformat(),
        "data": {"segments": [{"start_s": 0.0, "end_s": 1.0, "text": "hello"}]},
    }
    encoded = json.dumps(sidecar)
    decoded = json.loads(encoded)
    missing = REQUIRED_SIDECAR_KEYS.difference(decoded)
    if missing:
        fail(f"synthetic sidecar missing keys: {sorted(missing)}")
    if not isinstance(decoded["data"], dict):
        fail("synthetic sidecar data must be an object")


def main() -> int:
    package_names = check_workspace_members()
    check_package_layout(package_names)
    check_sidecar_schema()
    print(
        "python smoke passed: "
        f"{len(package_names)} workspace packages, sidecar schema keys present"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
