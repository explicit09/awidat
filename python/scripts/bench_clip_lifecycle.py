#!/usr/bin/env python3
"""Benchmark the real six-asset CLIP lifecycle through montage-index-perf."""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import hashlib
import json
import math
import os
import platform
import re
import signal
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

from bench_audio_energy import (
    MAX_SAMPLE_GAP_SECONDS,
    MIN_SAMPLE_RATE_HZ,
    SAMPLE_INTERVAL_SECONDS,
    BenchError,
    aggregate_process_tree,
    atomic_write_json,
    binary_provenance,
    canonical_data,
    controlled_environment as base_controlled_environment,
    filesystem_provenance,
    read_json,
    resolve_executable,
    run_text,
    sha256_file,
    summarize,
    tool_provenance,
    validate_benchmark_output_locations,
    validate_sampler_timing,
    wait_for_no_orphans,
    workspace_manifest,
)


ROOT = Path(__file__).resolve().parents[2]
ASSET_COUNT = 6
DEFAULT_SAMPLES = 7
LABEL_RE = re.compile(r"^[A-Za-z0-9_-]+$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
MODEL_REPOSITORY = "models--timm--vit_base_patch32_clip_224.openai"
MODEL_REVISION = "a6f597a30f7b82c51704746581f9a4e41421e878"
MODEL_FILENAME = "open_clip_model.safetensors"
MODEL_SHA256 = "e6d1bd7789aa45192b3bf90570a789b478bae1b74ebcce7eddd908e83a2b7c31"
MODEL_HF_HUB = "timm/vit_base_patch32_clip_224.openai/"
EXPECTED_MODEL = "ViT-B-32/openai"
EXPECTED_EMBEDDING_DIM = 512
EXPECTED_SAMPLE_FPS = 0.5
RUNTIME_PACKAGES = {
    "clip-mcp": "clip_mcp",
    "montage-mcp": "montage_mcp",
    "open-clip-torch": "open_clip",
    "torch": "torch",
    "torchvision": "torchvision",
    "numpy": "numpy",
}


def requested_asset_ids(assets: list[Path]) -> list[str]:
    return [f"external/{asset.name}" for asset in assets]


def validate_assets(values: list[str | Path]) -> list[Path]:
    if len(values) != ASSET_COUNT:
        raise BenchError(f"exactly {ASSET_COUNT} --asset paths are required")
    try:
        assets = [Path(value).expanduser().resolve(strict=True) for value in values]
    except OSError as error:
        raise BenchError(f"asset does not exist: {error}") from error
    if any(not asset.is_file() for asset in assets):
        raise BenchError("every --asset must name an existing file")
    if len(set(assets)) != ASSET_COUNT:
        raise BenchError("all six --asset paths must be distinct")
    if len(set(requested_asset_ids(assets))) != ASSET_COUNT:
        raise BenchError("asset filenames must be distinct for montage-index-perf staging")
    return assets


def resolve_python_workspace(value: str | Path) -> Path:
    try:
        root = Path(value).expanduser().resolve(strict=True)
    except OSError as error:
        raise BenchError(f"python workspace does not exist: {value}: {error}") from error
    required = (
        root / "pyproject.toml",
        root / "packages/clip-mcp/src/clip_mcp/__init__.py",
        root / "packages/montage-mcp/src/montage_mcp/__init__.py",
        root / ".venv/bin/python",
    )
    if any(not path.is_file() for path in required):
        raise BenchError(f"not an existing isolated CLIP workspace: {root}")
    return root


def python_launcher(python_root: Path) -> Path:
    launcher = python_root / ".venv/bin/python"
    if not launcher.is_file() or not os.access(launcher, os.X_OK):
        raise BenchError(f"supplied Python environment has no executable launcher: {launcher}")
    return launcher


def controller_python_provenance() -> dict[str, Any]:
    executable = resolve_executable(sys.executable)
    return {
        "sys_executable": sys.executable,
        "resolved_binary": binary_provenance(executable),
        "sys_version": sys.version,
    }


def model_provenance(value: str | Path) -> tuple[Path, Path, dict[str, Any]]:
    snapshot = Path(value).expanduser()
    if not snapshot.is_absolute():
        snapshot = Path.cwd() / snapshot
    try:
        weights = snapshot.resolve(strict=True)
    except OSError as error:
        raise BenchError(f"model weight file does not exist: {value}: {error}") from error
    if (
        not snapshot.is_symlink()
        or not snapshot.is_file()
        or snapshot.name != MODEL_FILENAME
        or snapshot.parent.name != MODEL_REVISION
        or snapshot.parent.parent.name != "snapshots"
        or snapshot.parent.parent.parent.name != MODEL_REPOSITORY
        or snapshot.parent.parent.parent.parent.name != "hub"
    ):
        raise BenchError(
            "model weights must be the pinned Hugging Face snapshot symlink for ViT-B-32/OpenAI"
        )
    digest = sha256_file(weights)
    if digest != MODEL_SHA256:
        raise BenchError(f"model SHA-256 does not match pinned ViT-B-32/OpenAI weights: {digest}")
    stat = weights.stat()
    hf_home = snapshot.parent.parent.parent.parent.parent
    return snapshot, hf_home, {
        "snapshot_path": str(snapshot),
        "resolved_blob": {
            "path": str(weights), "sha256": digest, "size_bytes": stat.st_size, "mtime_ns": stat.st_mtime_ns,
        },
        "expected_sha256": MODEL_SHA256,
        "hf_home": str(hf_home),
        "repository": MODEL_HF_HUB,
        "revision": MODEL_REVISION,
    }


def asset_fingerprint(path: Path) -> str:
    stat = path.stat()
    modified_seconds, modified_nanos = (
        divmod(stat.st_mtime_ns, 1_000_000_000) if stat.st_mtime_ns >= 0 else (0, 0)
    )
    identity = (
        f"montage-asset-fingerprint-v1\0{stat.st_size}\0{modified_seconds}\0{modified_nanos}"
    )
    return hashlib.sha256(identity.encode("utf-8")).hexdigest()


def clip_execution_inputs_provenance(
    binary: Path,
    assets: list[Path],
    asset_ids: list[str],
    model: dict[str, Any],
    controller_python: dict[str, Any],
    *,
    uv: Path,
    ffmpeg: Path,
    ffprobe: Path,
) -> dict[str, Any]:
    if len(assets) != ASSET_COUNT or len(asset_ids) != ASSET_COUNT:
        raise BenchError("CLIP execution inputs require exactly six assets")
    return {
        "dispatcher_binary": binary_provenance(binary),
        "controller_python": controller_python,
        "assets": {
            asset_id: {
                "asset_fingerprint": asset_fingerprint(asset),
                "content": binary_provenance(asset),
            }
            for asset, asset_id in zip(assets, asset_ids, strict=True)
        },
        "model": model,
        "tools": {
            "uv": tool_provenance(uv, ["--version"]),
            "ffmpeg": tool_provenance(ffmpeg, ["-version"]),
            "ffprobe": tool_provenance(ffprobe, ["-version"]),
        },
    }


def observe_clip_fixture(
    asset: Path,
    *,
    ffmpeg: Path,
    ffprobe: Path,
    timeout_seconds: float,
) -> dict[str, Any]:
    duration_command = [
        str(ffprobe),
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "format=duration",
        "-of",
        "default=noprint_wrappers=1:nokey=1",
        str(asset),
    ]
    raw_duration = run_text(duration_command, timeout=timeout_seconds).strip()
    try:
        duration_s = float(raw_duration)
    except ValueError as error:
        raise BenchError(f"invalid fixture duration for {asset}: {raw_duration!r}") from error
    if not math.isfinite(duration_s) or duration_s <= 0:
        raise BenchError(f"invalid fixture duration for {asset}: {raw_duration!r}")

    frame_command = [
        str(ffmpeg),
        "-nostdin",
        "-v",
        "error",
        "-i",
        str(asset),
        "-map",
        "0:v:0",
        "-vf",
        f"fps={EXPECTED_SAMPLE_FPS:g},scale=224:224",
        "-progress",
        "pipe:1",
        "-nostats",
        "-f",
        "null",
        "-",
    ]
    progress = run_text(frame_command, timeout=timeout_seconds)
    frame_values: list[int] = []
    progress_ended = False
    for line in progress.splitlines():
        key, separator, value = line.partition("=")
        if not separator:
            continue
        if key == "frame":
            try:
                frame_values.append(int(value.strip()))
            except ValueError as error:
                raise BenchError(f"invalid FFmpeg frame progress for {asset}: {line!r}") from error
        elif key == "progress" and value.strip() == "end":
            progress_ended = True
    if not progress_ended or not frame_values or frame_values[-1] <= 0:
        raise BenchError(f"incomplete FFmpeg frame-count oracle for {asset}")
    return {
        "duration_s": duration_s,
        "frame_count": frame_values[-1],
        "sample_fps": EXPECTED_SAMPLE_FPS,
        "duration_probe_command": duration_command,
        "frame_count_oracle_command": frame_command,
    }


def parse_runtime_preflight(raw: str, python_root: Path, weights: Path) -> dict[str, Any]:
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as error:
        raise BenchError(f"runtime preflight returned invalid JSON: {error}") from error
    if not isinstance(value, dict) or not isinstance(value.get("python"), str):
        raise BenchError("runtime preflight is incomplete")
    if value.get("clip_model") != EXPECTED_MODEL:
        raise BenchError("runtime preflight did not import the pinned CLIP model")
    reported_launcher = value.get("executable")
    if not isinstance(reported_launcher, str) or not reported_launcher:
        raise BenchError("runtime preflight executable is missing")
    launcher = Path(reported_launcher)
    expected_launcher = python_launcher(python_root)
    if launcher.absolute() != expected_launcher.absolute():
        raise BenchError(f"runtime preflight launched {launcher}, expected {expected_launcher}")
    executable = resolve_executable(launcher)
    if executable != resolve_executable(expected_launcher):
        raise BenchError(f"runtime preflight used {executable}, expected {expected_launcher}")
    packages = value.get("packages")
    if not isinstance(packages, dict) or set(packages) != set(RUNTIME_PACKAGES):
        raise BenchError("runtime preflight package set is incomplete")
    local_modules = {
        "clip-mcp": python_root / "packages/clip-mcp/src/clip_mcp/__init__.py",
        "montage-mcp": python_root / "packages/montage-mcp/src/montage_mcp/__init__.py",
    }
    venv = (python_root / ".venv").resolve(strict=True)
    runtime_packages: dict[str, dict[str, str]] = {}
    for name in RUNTIME_PACKAGES:
        package = packages[name]
        if (
            not isinstance(package, dict)
            or not isinstance(package.get("version"), str)
            or not package["version"]
            or not isinstance(package.get("module_path"), str)
            or not package["module_path"]
        ):
            raise BenchError(f"runtime preflight package is incomplete: {name}")
        try:
            module = Path(package["module_path"]).resolve(strict=True)
        except OSError as error:
            raise BenchError(f"runtime preflight package path is invalid: {name}: {error}") from error
        if name in local_modules:
            expected = local_modules[name].resolve(strict=True)
            if module != expected:
                raise BenchError(f"{name} resolved from {module}, expected {expected}")
        elif not module.is_relative_to(venv):
            raise BenchError(f"{name} resolved outside supplied .venv: {module}")
        runtime_packages[name] = {"version": package["version"], "path": str(module)}
    pretrained = value.get("pretrained")
    if (
        not isinstance(pretrained, dict)
        or pretrained.get("hf_hub") != MODEL_HF_HUB
        or not isinstance(pretrained.get("resolved_path"), str)
    ):
        raise BenchError("runtime preflight did not resolve the pinned OpenCLIP configuration")
    try:
        resolved = Path(pretrained["resolved_path"]).resolve(strict=True)
        expected_weights = weights.resolve(strict=True)
    except OSError as error:
        raise BenchError(f"runtime preflight artifact path is invalid: {error}") from error
    if resolved != expected_weights:
        raise BenchError(f"runtime preflight resolved {resolved}, expected {expected_weights}")
    digest = sha256_file(resolved)
    if digest != MODEL_SHA256:
        raise BenchError(f"runtime preflight resolved an unexpected model SHA-256: {digest}")
    return {
        "python_version": value["python"],
        "executable": {**binary_provenance(launcher), "resolved_path": str(executable)},
        "packages": runtime_packages,
        "offline_artifact_resolver": {
            "hf_hub": MODEL_HF_HUB,
            "snapshot_path": str(weights),
            "resolved_path": str(resolved),
            "sha256": digest,
        },
    }


def runtime_preflight(python_root: Path, hf_home: Path, weights: Path) -> dict[str, Any]:
    launcher = python_launcher(python_root)
    script = """
import importlib.metadata
import json
import pathlib
import sys

import clip_mcp
import montage_mcp
import numpy
import open_clip
import torch
import torchvision

cfg = open_clip.get_pretrained_cfg("ViT-B-32", "openai")
resolved = open_clip.download_pretrained(cfg, prefer_hf_hub=True)
modules = {
    "clip-mcp": clip_mcp,
    "montage-mcp": montage_mcp,
    "open-clip-torch": open_clip,
    "torch": torch,
    "torchvision": torchvision,
    "numpy": numpy,
}
print(json.dumps({
    "python": sys.version,
    "executable": sys.executable,
    "clip_model": f"{clip_mcp.MODEL_ARCH}/{clip_mcp.MODEL_PRETRAINED}",
    "packages": {
        name: {
            "version": importlib.metadata.version(name),
            "module_path": module.__file__,
        }
        for name, module in modules.items()
    },
    "pretrained": {
        "hf_hub": cfg.get("hf_hub"),
        "resolved_path": str(pathlib.Path(resolved).resolve()),
    },
}))
"""
    environment = os.environ.copy()
    environment.pop("PYTHONPATH", None)
    environment.update(
        {
            "HF_HOME": str(hf_home),
            "HF_HUB_OFFLINE": "1",
            "TRANSFORMERS_OFFLINE": "1",
            "PYTHONNOUSERSITE": "1",
            "PYTHONDONTWRITEBYTECODE": "1",
            "PYTHONSAFEPATH": "1",
        }
    )
    try:
        completed = subprocess.run(
            [str(launcher), "-c", script],
            cwd=python_root,
            env=environment,
            check=True,
            capture_output=True,
            text=True,
            timeout=120.0,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise BenchError(f"offline runtime preflight failed: {error}") from error
    return parse_runtime_preflight(completed.stdout, python_root, weights)


def git_provenance() -> dict[str, Any]:
    head = run_text(["git", "-C", str(ROOT), "rev-parse", "HEAD"]).strip()
    status = run_text(
        ["git", "-C", str(ROOT), "status", "--porcelain", "--untracked-files=all"]
    ).rstrip()
    if not head:
        raise BenchError("git returned an empty HEAD")
    if status:
        raise BenchError(f"refusing dirty source state:\n{status}")
    ignored = run_text(
        [
            "git",
            "-C",
            str(ROOT),
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--",
            "python",
        ]
    ).splitlines()
    unsafe_ignored = [
        path
        for path in ignored
        if not path.startswith("python/.venv/")
        and not ("/__pycache__/" in path and path.endswith(".pyc"))
    ]
    if unsafe_ignored:
        raise BenchError(
            "refusing dirty source state with ignored runtime inputs:\n"
            + "\n".join(unsafe_ignored)
        )
    return {
        "head": head,
        "git_status_clean": True,
        "ignored_venv_and_bytecode_bound_by_workspace_manifest": True,
    }


def source_provenance(python_root: Path) -> dict[str, dict[str, Any]]:
    sources = {
        "harness": Path(__file__),
        "dispatcher": ROOT / "crates/index/src/bin/montage-index-perf.rs",
        "dispatcher_core": ROOT / "crates/index/src/lib.rs",
        "clip_indexer": python_root / "packages/clip-mcp/src/clip_mcp/__init__.py",
        "montage_mcp": python_root / "packages/montage-mcp/src/montage_mcp/__init__.py",
        "python_lock": python_root / "uv.lock",
    }
    try:
        provenance = {
            name: binary_provenance(path.resolve(strict=True))
            for name, path in sources.items()
        }
    except OSError as error:
        raise BenchError(f"required source provenance is missing: {error}") from error
    provenance["python_workspace"] = workspace_manifest(python_root)
    return provenance


def unique_filesystems(paths: list[Path]) -> list[dict[str, Any]]:
    by_mount: dict[tuple[str, str, str, int], dict[str, Any]] = {}
    for path in paths:
        provenance = filesystem_provenance(path)
        key = tuple(
            provenance[name] for name in ("device", "mount", "filesystem_type", "st_dev")
        )
        by_mount[key] = provenance
    return [by_mount[key] for key in sorted(by_mount)]


def sampler_evidence(name: str, wall_seconds: float, raw_samples: list[dict[str, Any]]) -> dict[str, Any]:
    if wall_seconds <= 0 or not raw_samples:
        raise BenchError(f"{name} sampler is incomplete")
    gaps = [
        (current["elapsed_ms"] - previous["elapsed_ms"]) / 1000.0
        for previous, current in zip(raw_samples, raw_samples[1:])
    ]
    rate = len(raw_samples) / wall_seconds
    if rate < MIN_SAMPLE_RATE_HZ:
        raise BenchError(f"{name} sampler sample rate {rate:.2f}Hz was below {MIN_SAMPLE_RATE_HZ:.0f}Hz")
    timing = (
        validate_sampler_timing(
            name,
            wall_seconds=wall_seconds,
            sampler_count=len(raw_samples),
            sample_gaps_seconds=gaps,
        )
        if gaps
        else {"observed_rate_hz": rate, "gap_ms": None}
    )
    return {
        "target_interval_ms": SAMPLE_INTERVAL_SECONDS * 1000.0,
        "max_allowed_gap_ms": MAX_SAMPLE_GAP_SECONDS * 1000.0,
        "samples": len(raw_samples),
        **timing,
        "raw_samples": raw_samples,
    }


def _finite_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def validate_clip_sidecar(
    sidecar: Any,
    expected_asset_id: str,
    expected_asset_fingerprint: str,
    expected_observation: dict[str, Any] | None = None,
) -> dict[str, Any]:
    if not SHA256_RE.fullmatch(expected_asset_fingerprint):
        raise BenchError(f"invalid expected asset fingerprint for {expected_asset_id}")
    if not isinstance(sidecar, dict):
        raise BenchError("CLIP sidecar is not an object")
    header = ("indexer_version", "schema_version", "asset_sha256", "produced_at")
    if (
        sidecar.get("indexer") != "clip"
        or sidecar.get("asset_id") != expected_asset_id
        or any(not isinstance(sidecar.get(key), str) or not sidecar[key] for key in header)
        or not SHA256_RE.fullmatch(sidecar["asset_sha256"])
    ):
        raise BenchError(f"invalid CLIP sidecar header for {expected_asset_id}")
    if sidecar["asset_sha256"] != expected_asset_fingerprint:
        raise BenchError(f"CLIP sidecar asset fingerprint mismatch for {expected_asset_id}")
    data = sidecar.get("data")
    if not isinstance(data, dict):
        raise BenchError(f"CLIP sidecar has no data object for {expected_asset_id}")
    if data.get("model") != EXPECTED_MODEL:
        raise BenchError(f"CLIP sidecar did not use the pinned CLIP model for {expected_asset_id}")
    if data.get("embedding_dim") != EXPECTED_EMBEDDING_DIM:
        raise BenchError(
            f"CLIP sidecar was not 512-dimensional for {expected_asset_id}"
        )
    frames, dimension, timestamps = (
        data.get("frame_count"),
        data.get("embedding_dim"),
        data.get("timestamps_s"),
    )
    if (
        not isinstance(frames, int)
        or isinstance(frames, bool)
        or frames <= 0
        or not isinstance(dimension, int)
        or isinstance(dimension, bool)
        or dimension <= 0
        or not isinstance(timestamps, list)
        or len(timestamps) != frames
        or not all(_finite_number(timestamp) and timestamp >= 0 for timestamp in timestamps)
        or any(right <= left for left, right in zip(timestamps, timestamps[1:]))
        or data.get("embedding_dtype") != "float16"
        or data.get("embedding_encoding") != "base64"
        or data.get("frame_rate_sampled") != EXPECTED_SAMPLE_FPS
        or not _finite_number(data.get("duration_s"))
        or data["duration_s"] <= 0
        or not isinstance(data.get("embeddings_b64"), str)
    ):
        raise BenchError(f"invalid CLIP data for {expected_asset_id}")
    expected_timestamps = [index / EXPECTED_SAMPLE_FPS for index in range(frames)]
    if timestamps != expected_timestamps:
        raise BenchError(f"invalid CLIP timestamp grid for {expected_asset_id}")
    if expected_observation is not None:
        expected_frames = expected_observation.get("frame_count")
        expected_duration = expected_observation.get("duration_s")
        if (
            not isinstance(expected_frames, int)
            or isinstance(expected_frames, bool)
            or expected_frames <= 0
            or not _finite_number(expected_duration)
            or expected_duration <= 0
            or expected_observation.get("sample_fps") != EXPECTED_SAMPLE_FPS
        ):
            raise BenchError(f"invalid fixture observation for {expected_asset_id}")
        if frames != expected_frames:
            raise BenchError(
                f"CLIP frame count mismatch for {expected_asset_id}: "
                f"{frames} != {expected_frames}"
            )
        if not math.isclose(
            data["duration_s"], expected_duration, rel_tol=1e-9, abs_tol=1e-6
        ):
            raise BenchError(
                f"CLIP duration mismatch for {expected_asset_id}: "
                f"{data['duration_s']} != {expected_duration}"
            )
    try:
        encoded = base64.b64decode(data["embeddings_b64"], validate=True)
    except (TypeError, ValueError) as error:
        raise BenchError(f"invalid base64 float16 CLIP embeddings for {expected_asset_id}") from error
    if len(encoded) != frames * dimension * 2:
        raise BenchError(f"invalid float16 CLIP embeddings for {expected_asset_id}")
    try:
        finite = all(math.isfinite(value) for (value,) in struct.iter_unpack("<e", encoded))
    except struct.error as error:
        raise BenchError(f"invalid float16 CLIP embeddings for {expected_asset_id}") from error
    if not finite:
        raise BenchError(f"invalid float16 CLIP embeddings for {expected_asset_id}")
    digest = hashlib.sha256(encoded).hexdigest()
    stable_metadata = {
        **{key: sidecar[key] for key in sidecar if key not in {"produced_at", "data"}},
        **{key: value for key, value in data.items() if key not in {"perf", "embeddings_b64"}},
        "embedding_bytes_sha256": digest,
    }
    return {
        "asset_id": expected_asset_id,
        "timestamps_s": timestamps,
        "frame_count": frames,
        "model": data["model"],
        "embedding_dtype": data["embedding_dtype"],
        "embedding_encoding": data["embedding_encoding"],
        "embedding_dim": dimension,
        "embedding_bytes_sha256": digest,
        "stable_semantic_metadata": stable_metadata,
    }


def pair_telemetry(pair: dict[str, Any], asset_id: str) -> dict[str, Any]:
    timing_keys = ("queued_ms", "launch_init_ms", "tool_ms", "write_ms", "total_ms")
    if any(
        not isinstance(pair.get(key), int)
        or isinstance(pair[key], bool)
        or pair[key] < 0
        for key in timing_keys
    ):
        raise BenchError(f"dispatcher timing provenance is incomplete for {asset_id}")
    direct_rss = pair.get("peak_rss_bytes")
    if direct_rss is not None and (
        not isinstance(direct_rss, int) or isinstance(direct_rss, bool) or direct_rss < 0
    ):
        raise BenchError(f"dispatcher direct RSS provenance is invalid for {asset_id}")
    sidecar = pair.get("sidecar")
    perf = sidecar.get("perf") if isinstance(sidecar, dict) else None
    if (
        not isinstance(perf, dict)
        or not perf
        or any(
            not isinstance(key, str)
            or not key
            or not isinstance(value, int)
            or isinstance(value, bool)
            or value < 0
            for key, value in perf.items()
        )
    ):
        raise BenchError(f"sidecar perf timing provenance is incomplete for {asset_id}")
    return {
        "asset_id": asset_id,
        **{key: pair[key] for key in timing_keys},
        "direct_peak_rss_bytes": direct_rss,
        "sidecar_perf": perf,
    }


def validate_dispatcher_output(
    output_root: Path,
    run_label: str,
    expected_asset_ids: list[str],
    expected_asset_fingerprints: dict[str, str],
    expected_fixture_observations: dict[str, dict[str, Any]],
) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    if (
        set(expected_asset_fingerprints) != set(expected_asset_ids)
        or any(not SHA256_RE.fullmatch(value) for value in expected_asset_fingerprints.values())
        or set(expected_fixture_observations) != set(expected_asset_ids)
    ):
        raise BenchError("expected fixture evidence does not match the six requested asset ids")
    report_path = output_root / f"{run_label}-indexing-performance.json"
    document = read_json(report_path)
    try:
        command, report, pairs = document["command"], document["report"], document["report"]["pairs"]
    except (KeyError, TypeError) as error:
        raise BenchError(f"malformed dispatcher report: {report_path}") from error
    if (
        not isinstance(command, dict)
        or not isinstance(report, dict)
        or not isinstance(pairs, list)
        or command.get("included_indexers") != ["clip"]
        or command.get("concurrency") != 2
        or command.get("assets") != expected_asset_ids
        or any(report.get(key) != value for key, value in (("pair_count", 6), ("wrote", 6), ("skipped", 0), ("failed", 0), ("dep_skipped", 0)))
        or len(pairs) != ASSET_COUNT
        or any(
            not isinstance(pair, dict)
            or pair.get("indexer") != "clip"
            or pair.get("outcome") != "wrote"
            for pair in pairs
        )
        or {pair.get("asset_id") for pair in pairs} != set(expected_asset_ids)
    ):
        raise BenchError("dispatcher did not complete six CLIP writes")
    run_dirs = sorted(path for path in output_root.glob("index-run-*") if path.is_dir())
    if len(run_dirs) != 1:
        raise BenchError(f"expected one retained dispatcher sidecar root, got {run_dirs!r}")
    clip_root = run_dirs[0] / "clip"
    sidecars = {path.relative_to(clip_root).as_posix().removesuffix(".json"): path for path in clip_root.rglob("*.json") if path.is_file()}
    if len(sidecars) != ASSET_COUNT or set(sidecars) != set(expected_asset_ids):
        raise BenchError("retained CLIP sidecars did not match the six requested asset ids")
    pairs_by_asset = {pair["asset_id"]: pair for pair in pairs}
    records = [
        validate_clip_sidecar(
            read_json(sidecars[asset_id]),
            asset_id,
            expected_asset_fingerprints[asset_id],
            expected_fixture_observations[asset_id],
        )
        for asset_id in expected_asset_ids
    ]
    return records, {
        "dispatcher_report_path": str(report_path),
        "dispatcher_report_sha256": sha256_file(report_path),
        "dispatcher_summary": {key: report[key] for key in ("pair_count", "wrote", "skipped", "failed", "dep_skipped")},
        "retained_sidecars": {asset_id: str(sidecars[asset_id]) for asset_id in expected_asset_ids},
        "pair_telemetry": [
            pair_telemetry(pairs_by_asset[asset_id], asset_id) for asset_id in expected_asset_ids
        ],
    }


def controlled_environment(
    sample_root: Path, python_root: Path, hf_home: Path, uv: Path, ffmpeg: Path, ffprobe: Path
) -> dict[str, str]:
    environment = base_controlled_environment(sample_root, ffmpeg, ffprobe, uv)
    environment.pop("HF_TOKEN", None)
    environment.pop("PYTHONPATH", None)
    environment.update(
        {
            "HF_HOME": str(hf_home),
            "MONTAGE_PYTHON_ROOT": str(python_root),
            "HF_HUB_OFFLINE": "1",
            "TRANSFORMERS_OFFLINE": "1",
            "CLIP_SAMPLE_FPS": "0.5",
            "PYTHONNOUSERSITE": "1",
            "PYTHONDONTWRITEBYTECODE": "1",
            "PYTHONSAFEPATH": "1",
            "PYTHONHASHSEED": "0",
            "LC_ALL": "C",
            "TZ": "UTC",
            "UV_OFFLINE": "1",
            "UV_NO_SYNC": "1",
            "UV_PROJECT_ENVIRONMENT": str(python_root / ".venv"),
        }
    )
    return environment


ProcessRow = tuple[int, int, int, int, str]


def process_snapshot() -> list[ProcessRow]:
    try:
        completed = subprocess.run(
            ["ps", "-axo", "pid=,pgid=,ppid=,rss=,state="],
            check=True,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise BenchError(f"process-group sampler failed: {error}") from error
    rows: list[ProcessRow] = []
    for line in completed.stdout.splitlines():
        fields = line.split()
        if len(fields) < 5:
            continue
        try:
            pid, pgid, ppid, rss_kib = (int(value) for value in fields[:4])
        except ValueError:
            continue
        rows.append((pid, pgid, ppid, rss_kib, fields[4]))
    if not rows:
        raise BenchError("process-group sampler returned no parseable rows")
    return rows


def process_group_members(pgid: int) -> list[ProcessRow]:
    return [row for row in process_snapshot() if row[1] == pgid]


def wait_for_process_group_absent(
    pgid: int, process: subprocess.Popen[Any] | None = None, timeout_seconds: float = 3.0
) -> list[ProcessRow]:
    deadline = time.monotonic() + timeout_seconds
    while True:
        members = process_group_members(pgid)
        if process is None:
            active = members
        elif process.returncode is not None:
            return members
        else:
            active = [
                row
                for row in members
                if row[0] != process.pid or not row[4].startswith("Z")
            ]
            if not active:
                try:
                    process.wait(timeout=0.1)
                except subprocess.TimeoutExpired:
                    raise BenchError(
                        f"process-group sampler is missing live dispatcher {process.pid}"
                    ) from None
                else:
                    return []
        if not active or time.monotonic() >= deadline:
            return active
        time.sleep(0.05)


def terminate_process_group(
    pgid: int, process: subprocess.Popen[Any]
) -> list[ProcessRow]:
    if process.returncode is not None:
        return process_group_members(pgid)
    cleanup_error: BenchError | None = None
    members: list[ProcessRow] | None = None
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(pgid, sig)
        except ProcessLookupError:
            pass
        try:
            members = wait_for_process_group_absent(pgid, process)
        except BenchError as error:
            cleanup_error = error
            continue
        if not members:
            return []
        if process.returncode is not None:
            return members
    if process.returncode is None:
        try:
            process.wait(timeout=3)
        except (OSError, subprocess.TimeoutExpired) as error:
            raise BenchError(f"reap dispatcher after SIGKILL: {error}") from error
    if cleanup_error is not None:
        raise BenchError(f"verify dispatcher process-group cleanup: {cleanup_error}")
    return members or []


def run_sample(
    *,
    name: str,
    session_root: Path,
    binary: Path,
    assets: list[Path],
    asset_fingerprints: dict[str, str],
    fixture_observations: dict[str, dict[str, Any]],
    python_root: Path,
    hf_home: Path,
    uv: Path,
    ffmpeg: Path,
    ffprobe: Path,
    timeout_seconds: float,
) -> tuple[dict[str, Any], bytes]:
    sample_root = session_root / name
    sample_root.mkdir(parents=True, exist_ok=False)
    work_root, output_root = sample_root / "work", sample_root / "output"
    output_root.mkdir()
    command = [str(binary), *sum((["--asset", str(asset)] for asset in assets), [])]
    command += [
        "--output-dir", str(output_root), "--work-dir", str(work_root), "--label", name.replace("-", "_"),
        "--concurrency", "2", "--indexers", "clip",
    ]
    stdout_path, stderr_path = sample_root / "stdout.log", sample_root / "stderr.log"
    expected_asset_ids = requested_asset_ids(assets)
    observed_pids: set[int] = set()
    raw_samples: list[dict[str, Any]] = []
    peak_rss_bytes, started = 0, time.perf_counter()
    dispatcher_status: int | None = None
    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        try:
            process = subprocess.Popen(
                command,
                cwd=ROOT,
                env=controlled_environment(sample_root, python_root, hf_home, uv, ffmpeg, ffprobe),
                stdin=subprocess.DEVNULL,
                stdout=stdout,
                stderr=stderr,
                start_new_session=True,
            )
        except OSError as error:
            raise BenchError(f"spawn dispatcher: {error}") from error
        pgid = process.pid
        try:
            while True:
                sampled_at = time.perf_counter()
                rows = process_snapshot()
                tree_rows = [(pid, ppid, rss_kib) for pid, _, ppid, rss_kib, _ in rows]
                tree_pids, _ = aggregate_process_tree(process.pid, tree_rows)
                rss_by_pid = {pid: rss_kib for pid, _, _, rss_kib, _ in rows}
                group_pids = {pid for pid, row_pgid, _, _, _ in rows if row_pgid == pgid}
                live_pids = (tree_pids | group_pids) & set(rss_by_pid)
                if live_pids:
                    observed_pids.update(live_pids)
                    rss_bytes = sum(rss_by_pid[pid] * 1024 for pid in live_pids)
                    peak_rss_bytes = max(peak_rss_bytes, rss_bytes)
                    raw_samples.append({
                        "elapsed_ms": (sampled_at - started) * 1000.0,
                        "pids": sorted(live_pids),
                        "process_count": len(live_pids),
                        "rss_bytes": rss_bytes,
                        "process_group_pids": sorted(group_pids),
                    })
                if sampled_at - started > timeout_seconds:
                    raise BenchError(f"{name} exceeded timeout of {timeout_seconds:.1f}s")
                leader = next((row for row in rows if row[0] == process.pid), None)
                if leader is None:
                    raise BenchError(
                        f"{name} sampler lost the unreaped dispatcher PID {process.pid}"
                    )
                if leader[4].startswith("Z"):
                    lingering = wait_for_process_group_absent(pgid, process)
                    if lingering:
                        remaining = terminate_process_group(pgid, process)
                        dispatcher_status = process.returncode
                        if remaining:
                            raise BenchError(
                                f"{name} cleanup failed: process_group_members={remaining}"
                            )
                        if dispatcher_status == 0:
                            raise BenchError(
                                f"{name} required forced cleanup after a successful dispatcher "
                                f"exit: process_group_members={lingering}"
                            )
                    else:
                        dispatcher_status = process.returncode
                    if dispatcher_status is None:
                        raise BenchError(f"{name} dispatcher was not reaped after exit")
                    break
                time.sleep(max(0.0, SAMPLE_INTERVAL_SECONDS - (time.perf_counter() - sampled_at)))
        except BaseException as error:
            if process.returncode is None:
                remaining = terminate_process_group(pgid, process)
            else:
                remaining = process_group_members(pgid)
            if remaining:
                raise BenchError(f"{name} cleanup failed: process_group_members={remaining}") from error
            raise
    wall_seconds = time.perf_counter() - started
    if dispatcher_status != 0:
        tail = stderr_path.read_text(encoding="utf-8", errors="replace")[-4000:]
        raise BenchError(f"{name} dispatcher exited {dispatcher_status}: {tail}")
    if peak_rss_bytes <= 0:
        raise BenchError(f"{name} process-tree sampler did not observe positive RSS")
    remaining = process_group_members(pgid)
    if remaining:
        raise BenchError(f"{name} cleanup failed: process_group_members={remaining}")
    # The production dispatcher, MCP workers, and FFmpeg descendants inherit this
    # process group. Never signal an escaped bare PID: macOS exposes no race-free
    # process handle, so PID reuse could target an unrelated process.
    orphans = wait_for_no_orphans(observed_pids - {process.pid})
    if orphans:
        raise BenchError(
            f"{name} cleanup failed: observed descendant PIDs remain outside the "
            f"dispatcher process group and are not signaled by bare PID: {orphans}"
        )
    records, dispatcher = validate_dispatcher_output(
        output_root,
        name.replace("-", "_"),
        expected_asset_ids,
        asset_fingerprints,
        fixture_observations,
    )
    semantics, digest = canonical_data([record["stable_semantic_metadata"] for record in records])
    return {
        "name": name,
        "command": command,
        "wall_ms": wall_seconds * 1000.0,
        "process_tree_peak_rss_bytes": peak_rss_bytes,
        "sampler": sampler_evidence(name, wall_seconds, raw_samples),
        "cleanup": {
            "containment_scope": "dispatcher process group",
            "launcher": "start_new_session",
            "bare_pid_signaling": False,
            "process_group_pgid": pgid,
            "orphan_pids": [],
            "process_group_members": [],
            "passed": True,
            "retained_work_root": str(work_root),
        },
        "logs": {"stdout": binary_provenance(stdout_path), "stderr": binary_provenance(stderr_path)},
        "clip_assets": records,
        "semantic_signature_sha256": digest,
        **dispatcher,
    }, semantics


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--asset", action="append", required=True, help="one of six existing videos")
    parser.add_argument("--binary", default=os.environ.get("MONTAGE_INDEX_PERF_BINARY", ROOT / "target/release/montage-index-perf"))
    parser.add_argument("--python-root", default=os.environ.get("MONTAGE_PYTHON_ROOT"))
    parser.add_argument("--model-weights", default=os.environ.get("MONTAGE_CLIP_MODEL_WEIGHTS"))
    parser.add_argument("--samples", type=int, default=DEFAULT_SAMPLES)
    parser.add_argument("--timeout-seconds", type=float, default=1800.0)
    parser.add_argument("--work-root", type=Path, default=Path(tempfile.gettempdir()) / "montage-clip-lifecycle")
    parser.add_argument("--evidence-dir", type=Path)
    parser.add_argument("--label", default="clip-lifecycle")
    args = parser.parse_args(argv)
    if args.python_root is None or args.model_weights is None:
        parser.error("--python-root and --model-weights are required")
    if (
        args.samples < 5
        or not math.isfinite(args.timeout_seconds)
        or args.timeout_seconds <= 0
    ):
        parser.error("--samples must be at least 5 and --timeout-seconds must be positive")
    if not LABEL_RE.fullmatch(args.label):
        parser.error("--label must contain only letters, digits, '-' or '_'")
    args.work_root = args.work_root.expanduser().resolve()
    args.evidence_dir = args.evidence_dir.expanduser().resolve() if args.evidence_dir else args.work_root / "evidence"
    return args


def run_benchmark(args: argparse.Namespace) -> Path:
    assets = validate_assets(args.asset)
    git = git_provenance()
    controller_python = controller_python_provenance()
    binary, python_root = resolve_executable(args.binary), resolve_python_workspace(args.python_root)
    output_locations = {
        "work root": args.work_root,
        "evidence directory": args.evidence_dir,
        "run directory": args.work_root / "runs",
    }
    validate_benchmark_output_locations(
        python_root,
        output_locations,
        repository_root=ROOT,
    )
    weights, hf_home, model = model_provenance(args.model_weights)
    runtime = runtime_preflight(python_root, hf_home, weights)
    sources = source_provenance(python_root)
    uv = resolve_executable(os.environ.get("MONTAGE_UV", "uv"))
    ffmpeg = resolve_executable(os.environ.get("MONTAGE_FFMPEG", "ffmpeg"))
    ffprobe = resolve_executable(os.environ.get("MONTAGE_FFPROBE", "ffprobe"))
    args.work_root.mkdir(parents=True, exist_ok=True)
    args.evidence_dir.mkdir(parents=True, exist_ok=True)
    session_id = f"{args.label}-{dt.datetime.now(dt.UTC).strftime('%Y%m%dT%H%M%S')}-{os.getpid()}-{time.time_ns()}"
    session_root = args.work_root / "runs" / session_id
    session_root.mkdir(parents=True, exist_ok=False)
    asset_ids = requested_asset_ids(assets)
    execution_inputs = clip_execution_inputs_provenance(
        binary,
        assets,
        asset_ids,
        model,
        controller_python,
        uv=uv,
        ffmpeg=ffmpeg,
        ffprobe=ffprobe,
    )
    fixture_observations = {
        asset_id: observe_clip_fixture(
            asset,
            ffmpeg=ffmpeg,
            ffprobe=ffprobe,
            timeout_seconds=args.timeout_seconds,
        )
        for asset, asset_id in zip(assets, asset_ids, strict=True)
    }
    _observed_weights, _observed_hf_home, observed_model = model_provenance(
        args.model_weights
    )
    observed_execution_inputs = clip_execution_inputs_provenance(
        binary,
        assets,
        asset_ids,
        observed_model,
        controller_python_provenance(),
        uv=uv,
        ffmpeg=ffmpeg,
        ffprobe=ffprobe,
    )
    if observed_execution_inputs != execution_inputs:
        raise BenchError("benchmark execution inputs changed during fixture observation")
    asset_fingerprints = {
        asset_id: execution_inputs["assets"][asset_id]["asset_fingerprint"]
        for asset_id in asset_ids
    }
    fixtures: list[dict[str, Any]] = []
    for asset, asset_id in zip(assets, asset_ids, strict=True):
        content = execution_inputs["assets"][asset_id]["content"]
        fixtures.append(
            {
                "asset_id": asset_id,
                "asset_fingerprint": asset_fingerprints[asset_id],
                "clip_observation": fixture_observations[asset_id],
                "content_sha256": content["sha256"],
                **content,
                "filesystem": filesystem_provenance(asset),
            }
        )
    common = {
        "session_root": session_root, "binary": binary, "assets": assets, "python_root": python_root,
        "asset_fingerprints": asset_fingerprints,
        "fixture_observations": fixture_observations,
        "hf_home": hf_home,
        "uv": uv, "ffmpeg": ffmpeg, "ffprobe": ffprobe,
        "timeout_seconds": args.timeout_seconds,
    }
    warmup, baseline = run_sample(name="warmup-00", **common)
    samples: list[dict[str, Any]] = []
    for index in range(1, args.samples + 1):
        sample, semantics = run_sample(name=f"sample-{index:02d}", **common)
        if semantics != baseline:
            raise BenchError(
                f"sample-{index:02d} CLIP output differs from warmup: "
                f"{sample['semantic_signature_sha256']} != {warmup['semantic_signature_sha256']}"
            )
        samples.append(sample)
    final_git = git_provenance()
    if final_git != git:
        raise BenchError(f"Git state changed during benchmark: {git!r} != {final_git!r}")
    final_sources = source_provenance(python_root)
    if final_sources != sources:
        raise BenchError("Python workspace changed during benchmark")
    _final_weights, _final_hf_home, final_model = model_provenance(
        args.model_weights
    )
    final_execution_inputs = clip_execution_inputs_provenance(
        binary,
        assets,
        asset_ids,
        final_model,
        controller_python_provenance(),
        uv=uv,
        ffmpeg=ffmpeg,
        ffprobe=ffprobe,
    )
    if final_execution_inputs != execution_inputs:
        raise BenchError("benchmark execution inputs changed during samples")
    report = {
        "schema_version": 1,
        "generated_at_utc": dt.datetime.now(dt.UTC).isoformat(),
        "configuration": {
            "label": args.label, "warmups": 1, "samples": args.samples, "asset_count": ASSET_COUNT,
            "dispatcher_indexers": ["clip"], "dispatcher_concurrency": 2,
            "sample_interval_ms": SAMPLE_INTERVAL_SECONDS * 1000.0,
        },
        "fixtures": fixtures,
        "provenance": {
            "dispatcher_binary": execution_inputs["dispatcher_binary"],
            "controller_python": execution_inputs["controller_python"],
            "python_runtime": {
                "workspace": str(python_root),
                **runtime,
            },
            "model": execution_inputs["model"],
            "tools": execution_inputs["tools"],
            "sources": sources,
            "git": git,
            "filesystems": {
                "work": filesystem_provenance(args.work_root),
                "evidence": filesystem_provenance(args.evidence_dir),
                "fixture_mounts": unique_filesystems(assets),
                "model": filesystem_provenance(weights),
            },
            "machine": {"system": platform.system(), "release": platform.release(), "machine": platform.machine(), "cpu_count": os.cpu_count()},
            "controlled_environment": {"MONTAGE_PYTHON_ROOT": str(python_root), "HF_HOME": str(hf_home), "HF_HUB_OFFLINE": "1", "TRANSFORMERS_OFFLINE": "1", "CLIP_SAMPLE_FPS": "0.5", "PYTHONNOUSERSITE": "1", "PYTHONDONTWRITEBYTECODE": "1", "PYTHONSAFEPATH": "1", "PYTHONPATH": "unset", "PYTHONHASHSEED": "0", "LC_ALL": "C", "TZ": "UTC", "UV_OFFLINE": "1", "UV_NO_SYNC": "1", "UV_PROJECT_ENVIRONMENT": str(python_root / ".venv"), "PATH_PREFIX": [str(uv.parent), str(ffprobe.parent), str(ffmpeg.parent)]},
        },
        "correctness": {
            "expected_asset_ids": asset_ids,
            "asset_fingerprints": asset_fingerprints,
            "fixture_observations": fixture_observations,
            "warmup_semantic_signature_sha256": warmup["semantic_signature_sha256"],
            "all_timed_samples_exactly_match_warmup": True,
            "comparison_excludes": ["produced_at", "data.perf"],
            "comparison_includes": ["timestamps_s", "model", "embedding dtype/encoding/dimension", "decoded float16 embedding SHA-256"],
        },
        "warmup": warmup,
        "samples": samples,
        "statistics": {
            "wall_ms": summarize([sample["wall_ms"] for sample in samples]),
            "process_tree_peak_rss_bytes": summarize([float(sample["process_tree_peak_rss_bytes"]) for sample in samples]),
        },
        "cleanup": {"all_samples_passed": all(sample["cleanup"]["passed"] for sample in samples), "session_roots_retained_for_evidence": True},
    }
    output = args.evidence_dir / f"{session_id}-clip-lifecycle-performance.json"
    if output.exists():
        raise BenchError(f"refusing to overwrite evidence: {output}")
    atomic_write_json(output, report)
    return output


def main(argv: list[str] | None = None) -> int:
    try:
        output = run_benchmark(parse_args(argv))
    except BenchError as error:
        print(f"bench-clip-lifecycle: {error}", file=sys.stderr)
        return 1
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
