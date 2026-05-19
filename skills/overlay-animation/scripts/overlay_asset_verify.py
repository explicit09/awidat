#!/usr/bin/env python3
"""Verify generated overlay assets before timeline insertion."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


def _is_safe_project_path(value: str) -> bool:
    if not value:
        return False
    path = Path(value)
    return not path.is_absolute() and ".." not in path.parts


def _issue(slot_name: str, code: str, message: str, path: str | None = None) -> dict:
    payload = {
        "slot": slot_name,
        "code": code,
        "message": message,
    }
    if path is not None:
        payload["path"] = path
    return payload


def _path_status(
    *,
    slot_name: str,
    value: str,
    project_root: Path,
    unsafe_code: str,
    missing_code: str,
    label: str,
) -> tuple[str | None, list[dict]]:
    issues: list[dict] = []
    if not _is_safe_project_path(value):
        issues.append(_issue(
            slot_name,
            unsafe_code,
            f"{label} must be a non-empty project-relative path",
            value,
        ))
        return None, issues

    resolved = project_root / value
    if not resolved.is_file():
        issues.append(_issue(
            slot_name,
            missing_code,
            f"{label} does not exist",
            value,
        ))
    return resolved.as_posix(), issues


def _edl_field(edl_hint: str, field: str) -> str | None:
    prefix = f"+ {field}:"
    for line in edl_hint.splitlines():
        stripped = line.strip()
        if stripped.startswith(prefix):
            return stripped[len(prefix):].strip()
    return None


def _edl_hint_issues(
    *,
    slot_name: str,
    edl_hint: str,
    asset_path: str,
    duration_s: float,
) -> list[dict]:
    issues: list[dict] = []
    if not edl_hint.strip():
        return [_issue(
            slot_name,
            "missing_edl_hint",
            "slot must include an EDL insertion hint",
        )]

    hinted_asset = _edl_field(edl_hint, "asset")
    if hinted_asset != asset_path:
        issues.append(_issue(
            slot_name,
            "edl_asset_mismatch",
            "EDL hint asset must match asset_path",
            hinted_asset,
        ))

    hinted_duration = _edl_field(edl_hint, "duration_s")
    try:
        hinted_duration_s = float(hinted_duration or "")
    except ValueError:
        hinted_duration_s = -1.0
    if round(hinted_duration_s, 3) != round(duration_s, 3):
        issues.append(_issue(
            slot_name,
            "edl_duration_mismatch",
            "EDL hint duration_s must match duration_s",
            hinted_duration,
        ))
    return issues


def verify_slot(slot: dict[str, Any], *, project_root: Path) -> dict:
    name = str(slot.get("name", "Overlay")).strip() or "Overlay"
    issues: list[dict] = []
    asset_path = str(slot.get("asset_path", "")).strip()
    duration_s = float(slot.get("duration_s", 0.0))
    edl_hint = str(slot.get("edl_hint", "")).strip()

    _, asset_issues = _path_status(
        slot_name=name,
        value=asset_path,
        project_root=project_root,
        unsafe_code="unsafe_overlay_path",
        missing_code="missing_overlay_asset",
        label="overlay asset path",
    )
    issues.extend(asset_issues)

    if duration_s <= 0.0:
        issues.append(_issue(
            name,
            "invalid_duration",
            "duration_s must be positive",
        ))
    issues.extend(_edl_hint_issues(
        slot_name=name,
        edl_hint=edl_hint,
        asset_path=asset_path,
        duration_s=duration_s,
    ))

    matte_path = str(slot.get("matte_path", "")).strip()
    if bool(slot.get("subject_aware", False)):
        _, matte_issues = _path_status(
            slot_name=name,
            value=matte_path,
            project_root=project_root,
            unsafe_code="unsafe_matte_path",
            missing_code="missing_subject_matte",
            label="subject matte path",
        )
        issues.extend(matte_issues)

    return {
        "name": name,
        "asset_path": asset_path,
        "subject_aware": bool(slot.get("subject_aware", False)),
        "matte_path": matte_path if matte_path else None,
        "duration_s": round(duration_s, 3),
        "edl_hint_present": bool(edl_hint),
        "status": "blocked" if issues else "ready",
        "issues": issues,
    }


def verify_manifest(manifest: dict[str, Any], *, project_root: Path) -> dict:
    slots = manifest.get("slots", [])
    if not isinstance(slots, list):
        raise ValueError("manifest slots must be a list")

    slot_reports = [
        verify_slot(slot, project_root=project_root)
        for slot in slots
        if isinstance(slot, dict)
    ]
    issues = [
        issue
        for slot_report in slot_reports
        for issue in slot_report["issues"]
    ]
    return {
        "version": 1,
        "project_root": project_root.as_posix(),
        "status": "blocked" if issues else "ready",
        "issue_count": len(issues),
        "issues": issues,
        "slots": slot_reports,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--project-root", default=".")
    args = parser.parse_args()

    manifest_path = Path(args.manifest)
    manifest = json.loads(manifest_path.read_text())
    report = verify_manifest(manifest, project_root=Path(args.project_root))
    print(json.dumps(report, indent=2))
    if report["status"] != "ready":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
