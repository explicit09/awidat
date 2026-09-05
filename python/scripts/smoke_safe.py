#!/usr/bin/env python3
"""Safe Python indexer smoke checks.

This script is intentionally metadata/schema-only. It does not import
heavy indexer modules, run ffmpeg, download models, or contact gated
Hugging Face resources. Use python/SMOKE.md for the full manual smoke.
"""

from __future__ import annotations

import json
import os
import sys
import tomllib
from datetime import datetime, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = ROOT.parent
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
EVAL_WORKFLOW_REQUIRED_STRINGS = [
    "MONTAGE_REAL_CORPUS",
    "MONTAGE_REAL_VISUAL_MIN_METADATA_SHOTS",
    "MONTAGE_REAL_VISUAL_MIN_METADATA_RATIO",
    "MONTAGE_REAL_VISUAL_MIN_MATCH_CANDIDATE_SHOTS",
    "MONTAGE_REAL_VISUAL_MIN_MODEL_COMPOSITION_SHOTS",
    "MONTAGE_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS",
    "MONTAGE_REAL_VISUAL_MAX_INVALID_COMPOSITION_MODEL_REGIONS",
    "MONTAGE_REAL_MIN_ASSESSOR_PROPOSAL_FIXTURES",
    "MONTAGE_REAL_MIN_TRANSITION_PLANNER_FIXTURES",
    "MONTAGE_REAL_MIN_ROUGH_ASSEMBLY_FIXTURES",
    "MONTAGE_COMPOSITION_MODEL_PROJECT",
    "MONTAGE_COMPOSITION_MODEL_MIN_REGIONS",
    "MONTAGE_COMPOSITION_MODEL_MAX_INVALID_REGIONS",
    'test -n "$MONTAGE_REAL_CORPUS"',
    'test -d "$MONTAGE_REAL_CORPUS"',
    'test -f "$MONTAGE_REAL_CORPUS/project.otio.json"',
    "python3 python/scripts/smoke_safe.py",
    "env.MONTAGE_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS != '0'",
]
REAL_CORPUS_FIXTURE_GATES = [
    (
        "MONTAGE_REAL_MIN_ASSESSOR_PROPOSAL_FIXTURES",
        "assessor-proposal",
        ".montage/eval/assessor-proposal-flow.json",
        ".montage/eval/assessor-proposals",
    ),
    (
        "MONTAGE_REAL_MIN_TRANSITION_PLANNER_FIXTURES",
        "transition-planner",
        ".montage/eval/transition-planner-flow.json",
        ".montage/eval/transition-planners",
    ),
    (
        "MONTAGE_REAL_MIN_ROUGH_ASSEMBLY_FIXTURES",
        "rough-assembly",
        ".montage/eval/rough-assembly-flow.json",
        ".montage/eval/rough-assemblies",
    ),
]
COMPOSITION_MODEL_ALLOWED_VALUES = {
    "subject_role": {
        "environment",
        "background_person",
        "featured_subject",
        "primary_speaker",
        "secondary_subject",
        "object_detail",
    },
    "depth_layer": {"foreground", "midground", "background", "mixed_depth"},
    "framing": {
        "extreme_close_up",
        "single_close",
        "single_medium",
        "wide_context",
        "two_shot",
        "group",
        "insert",
    },
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
        if package_name == "montage-mcp":
            for exported in ["Sidecar", "IndexerServer", "IndexAssetRequest"]:
                if exported not in text:
                    fail(f"montage-mcp __init__.py does not export {exported}")
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


def _required_string(region: dict, key: str) -> str:
    value = region.get(key)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"composition-model region missing non-empty {key}")
    return value


def _required_choice(region: dict, key: str) -> str:
    value = _required_string(region, key)
    allowed = COMPOSITION_MODEL_ALLOWED_VALUES[key]
    if value not in allowed:
        raise ValueError(
            f"composition-model region {key} must be one of {sorted(allowed)}, got {value!r}"
        )
    return value


def _number(region: dict, key: str) -> float:
    value = region.get(key)
    if not isinstance(value, int | float):
        raise ValueError(f"composition-model region {key} must be numeric")
    return float(value)


def validate_composition_model_body(body: dict) -> None:
    regions = body.get("regions")
    if not isinstance(regions, list) or not regions:
        raise ValueError("composition-model data.regions must be a non-empty list")
    for region in regions:
        if not isinstance(region, dict):
            raise ValueError("composition-model region must be an object")
        start_s = _number(region, "start_s")
        end_s = _number(region, "end_s")
        if end_s <= start_s:
            raise ValueError("composition-model region end_s must be greater than start_s")
        source = _required_string(region, "composition_source")
        if not source.startswith("model:"):
            raise ValueError("composition-model composition_source must start with model:")
        confidence = _number(region, "composition_confidence")
        if not 0.0 <= confidence <= 1.0:
            raise ValueError("composition-model composition_confidence must be in [0, 1]")
        for key in ["subject_role", "depth_layer", "framing"]:
            _required_choice(region, key)


def check_composition_model_sidecar_schema() -> None:
    valid = {
        "regions": [
            {
                "start_s": 0.0,
                "end_s": 2.0,
                "composition_source": "model:composition-v2",
                "composition_confidence": 0.91,
                "subject_role": "primary_speaker",
                "depth_layer": "foreground",
                "framing": "single_close",
            }
        ]
    }
    invalid = {
        "regions": [
            {
                "start_s": 2.0,
                "end_s": 1.0,
                "composition_source": "heuristic:composition-v1",
                "composition_confidence": 1.5,
            }
        ]
    }
    validate_composition_model_body(valid)
    try:
        validate_composition_model_body(invalid)
    except ValueError:
        return
    fail("invalid composition-model sidecar passed validation")


def check_composition_model_sample_fixture() -> None:
    path = ROOT / "fixtures" / "composition-model" / "sample.json"
    if not path.is_file():
        fail(f"missing composition-model sample fixture: {path.relative_to(ROOT)}")
    sample = json.loads(path.read_text())
    missing = REQUIRED_SIDECAR_KEYS.difference(sample)
    if missing:
        fail(f"composition-model sample missing sidecar keys: {sorted(missing)}")
    if sample.get("indexer") != "composition-model":
        fail("composition-model sample indexer must be 'composition-model'")
    data = sample.get("data")
    if not isinstance(data, dict):
        fail("composition-model sample data must be an object")
    try:
        validate_composition_model_body(data)
    except ValueError as exc:
        fail(f"composition-model sample is invalid: {exc}")


def check_eval_workflow_contract() -> None:
    path = REPO_ROOT / ".github" / "workflows" / "evals.yml"
    if not path.is_file():
        fail(f"missing eval workflow: {path.relative_to(REPO_ROOT)}")
    workflow = path.read_text()
    missing = [needle for needle in EVAL_WORKFLOW_REQUIRED_STRINGS if needle not in workflow]
    if missing:
        fail(f"eval workflow missing real-corpus contract strings: {missing}")


def validate_composition_model_project_tree(
    project_root: Path, min_regions: int = 1, max_invalid_regions: int = 0
) -> tuple[int, int, int]:
    sidecar_root = project_root / "index" / "composition-model"
    if not sidecar_root.is_dir():
        raise ValueError(f"missing composition-model sidecar directory: {sidecar_root}")
    paths = sorted(path for path in sidecar_root.rglob("*.json") if path.is_file())
    if not paths:
        raise ValueError(f"composition-model sidecar directory has no JSON files: {sidecar_root}")

    region_count = 0
    invalid_regions: list[str] = []
    for path in paths:
        sidecar = json.loads(path.read_text())
        missing = REQUIRED_SIDECAR_KEYS.difference(sidecar)
        if missing:
            raise ValueError(
                f"{path.relative_to(project_root)} missing sidecar keys: {sorted(missing)}"
            )
        if sidecar.get("indexer") != "composition-model":
            raise ValueError(f"{path.relative_to(project_root)} indexer must be composition-model")
        data = sidecar.get("data")
        if not isinstance(data, dict):
            raise ValueError(f"{path.relative_to(project_root)} data must be an object")
        regions = data.get("regions")
        if not isinstance(regions, list) or not regions:
            raise ValueError(
                f"{path.relative_to(project_root)} composition-model data.regions "
                "must be a non-empty list"
            )
        for index, region in enumerate(regions):
            try:
                validate_composition_model_body({"regions": [region]})
            except ValueError as exc:
                invalid_regions.append(
                    f"{path.relative_to(project_root)} region {index}: {exc}"
                )
            else:
                region_count += 1

    if len(invalid_regions) > max_invalid_regions:
        raise ValueError(
            f"composition-model tree has {region_count} valid model region(s), "
            f"{len(invalid_regions)} invalid model region(s): "
            + "; ".join(invalid_regions[:5])
        )

    if region_count < min_regions:
        raise ValueError(
            f"composition-model tree has {region_count} model region(s), "
            f"below required minimum {min_regions}"
        )
    return len(paths), region_count, len(invalid_regions)


def _parse_env_int(name: str, raw_value: str | None, default: int, *, minimum: int) -> int:
    if raw_value is None or raw_value.strip() == "":
        return default
    try:
        value = int(raw_value)
    except ValueError as exc:
        fail(f"{name} must be an integer: {raw_value!r}")
        raise exc
    if value < minimum:
        fail(f"{name} must be at least {minimum}")
    return value


def _first_non_blank_env(*names: str) -> str | None:
    for name in names:
        value = os.environ.get(name)
        if value is not None and value.strip() != "":
            return value
    return None


def _any_non_blank_env(*names: str) -> bool:
    return _first_non_blank_env(*names) is not None


def _real_corpus_root_for_fixture_gates() -> Path | None:
    raw_root = _first_non_blank_env("MONTAGE_REAL_CORPUS")
    if raw_root is None:
        return None
    return Path(raw_root)


def _json_fixture_count(project_root: Path, default_file: str, directory: str) -> int:
    paths: set[Path] = set()
    default_path = project_root / default_file
    if default_path.is_file():
        paths.add(default_path)
    directory_path = project_root / directory
    if directory_path.is_dir():
        paths.update(
            path
            for path in directory_path.iterdir()
            if path.is_file() and path.suffix.lower() == ".json"
        )
    return len(paths)


def check_real_corpus_fixture_gates_from_env() -> dict[str, int]:
    configured = []
    for env_name, label, default_file, directory in REAL_CORPUS_FIXTURE_GATES:
        minimum = _parse_env_int(env_name, os.environ.get(env_name), 0, minimum=0)
        if minimum > 0:
            configured.append((env_name, label, default_file, directory, minimum))
    if not configured:
        return {}

    project_root = _real_corpus_root_for_fixture_gates()
    if project_root is None:
        fail("MONTAGE_REAL_CORPUS must be set when real-corpus fixture minimums are configured")
    if not project_root.is_dir():
        fail(f"MONTAGE_REAL_CORPUS must be an existing directory: {project_root}")
    if not (project_root / "project.otio.json").is_file():
        fail(f"MONTAGE_REAL_CORPUS must contain project.otio.json: {project_root}")

    counts: dict[str, int] = {}
    for env_name, label, default_file, directory, minimum in configured:
        count = _json_fixture_count(project_root, default_file, directory)
        counts[label] = count
        if count < minimum:
            fail(
                f"expected at least {minimum} {label} fixture(s) from {env_name}, "
                f"found {count}"
            )
    return counts


def _real_visual_min_regions_env() -> str | None:
    value = os.environ.get("MONTAGE_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS")
    if value is None or value.strip() in {"", "0"}:
        return None
    return value


def _composition_model_project_tree_gate_configured() -> bool:
    return _any_non_blank_env(
        "MONTAGE_COMPOSITION_MODEL_MIN_REGIONS",
        "MONTAGE_COMPOSITION_MODEL_MAX_INVALID_REGIONS",
        "MONTAGE_REAL_VISUAL_MAX_INVALID_COMPOSITION_MODEL_REGIONS",
    ) or _real_visual_min_regions_env() is not None


def check_composition_model_project_tree_from_env() -> tuple[int, int, int] | None:
    raw_root = _first_non_blank_env("MONTAGE_COMPOSITION_MODEL_PROJECT")
    if raw_root is None and _composition_model_project_tree_gate_configured():
        raw_root = _first_non_blank_env("MONTAGE_REAL_CORPUS")
    if not raw_root:
        if _composition_model_project_tree_gate_configured():
            fail(
                "MONTAGE_COMPOSITION_MODEL_PROJECT or MONTAGE_REAL_CORPUS must be set "
                "when composition-model project-tree thresholds are configured"
            )
        return None
    fallback_min_regions = _first_non_blank_env(
        "MONTAGE_COMPOSITION_MODEL_MIN_REGIONS",
    )
    if fallback_min_regions is None:
        fallback_min_regions = _real_visual_min_regions_env()
    min_regions = _parse_env_int(
        "MONTAGE_COMPOSITION_MODEL_MIN_REGIONS",
        fallback_min_regions,
        1,
        minimum=1,
    )
    fallback_max_invalid = _first_non_blank_env(
        "MONTAGE_COMPOSITION_MODEL_MAX_INVALID_REGIONS",
        "MONTAGE_REAL_VISUAL_MAX_INVALID_COMPOSITION_MODEL_REGIONS",
    )
    max_invalid_regions = _parse_env_int(
        "MONTAGE_COMPOSITION_MODEL_MAX_INVALID_REGIONS",
        fallback_max_invalid,
        0,
        minimum=0,
    )
    try:
        return validate_composition_model_project_tree(
            Path(raw_root),
            min_regions=min_regions,
            max_invalid_regions=max_invalid_regions,
        )
    except ValueError as exc:
        fail(f"composition-model project tree is invalid: {exc}")


def main() -> int:
    package_names = check_workspace_members()
    check_package_layout(package_names)
    check_sidecar_schema()
    check_composition_model_sidecar_schema()
    check_composition_model_sample_fixture()
    check_eval_workflow_contract()
    fixture_counts = check_real_corpus_fixture_gates_from_env()
    project_tree = check_composition_model_project_tree_from_env()
    project_tree_msg = ""
    if project_tree is not None:
        files, regions, invalid_regions = project_tree
        project_tree_msg = (
            f", composition-model project tree: {files} files / {regions} valid regions"
            f" / {invalid_regions} invalid regions"
        )
    fixture_msg = ""
    if fixture_counts:
        fixture_msg = ", real-corpus fixtures: " + ", ".join(
            f"{label}={count}" for label, count in sorted(fixture_counts.items())
        )
    print(
        "python smoke passed: "
        f"{len(package_names)} workspace packages, sidecar schema keys present, "
        "eval workflow contract present"
        f"{fixture_msg}"
        f"{project_tree_msg}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
