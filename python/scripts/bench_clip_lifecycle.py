#!/usr/bin/env python3
"""Benchmark the real six-asset CLIP lifecycle through montage-index-perf."""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import hashlib
import math
import os
import platform
import re
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
    sample_processes,
    sha256_file,
    summarize,
    terminate_process_group,
    tool_provenance,
    validate_sampler_timing,
    wait_for_no_orphans,
)


ROOT = Path(__file__).resolve().parents[2]
ASSET_COUNT = 6
DEFAULT_SAMPLES = 7
LABEL_RE = re.compile(r"^[A-Za-z0-9_-]+$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
MODEL_FILENAME = "ViT-B-32.pt"
MODEL_SHA256 = "40d365715913c9da98579312b702a82c18be219cc2a73407c4526f58eba950af"


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
        root / ".venv/bin/python",
    )
    if any(not path.is_file() for path in required):
        raise BenchError(f"not an existing isolated CLIP workspace: {root}")
    return root


def model_provenance(value: str | Path) -> tuple[Path, Path, dict[str, Any]]:
    try:
        weights = Path(value).expanduser().resolve(strict=True)
    except OSError as error:
        raise BenchError(f"model weight file does not exist: {value}: {error}") from error
    if (
        not weights.is_file()
        or weights.name != MODEL_FILENAME
        or weights.parent.name != "clip"
        or weights.parent.parent.name != ".cache"
    ):
        raise BenchError(
            "model weights must be <model-home>/.cache/clip/ViT-B-32.pt for offline OpenCLIP"
        )
    digest = sha256_file(weights)
    if digest != MODEL_SHA256:
        raise BenchError(f"model SHA-256 does not match pinned ViT-B-32/OpenAI weights: {digest}")
    return weights, weights.parent.parent.parent, {
        **binary_provenance(weights),
        "expected_sha256": MODEL_SHA256,
    }


def git_provenance() -> dict[str, Any]:
    head = run_text(["git", "-C", str(ROOT), "rev-parse", "HEAD"]).strip()
    status = run_text(
        ["git", "-C", str(ROOT), "status", "--porcelain", "--untracked-files=no"]
    ).rstrip()
    if not head:
        raise BenchError("git returned an empty HEAD")
    if status:
        raise BenchError(f"refusing dirty tracked source state:\n{status}")
    return {"head": head, "tracked_clean": True, "untracked_ignored_evidence_allowed": True}


def source_provenance(python_root: Path) -> dict[str, dict[str, Any]]:
    sources = {
        "harness": Path(__file__),
        "dispatcher": ROOT / "crates/index/src/bin/montage-index-perf.rs",
        "dispatcher_core": ROOT / "crates/index/src/lib.rs",
        "clip_indexer": python_root / "packages/clip-mcp/src/clip_mcp/__init__.py",
        "python_lock": python_root / "uv.lock",
    }
    try:
        return {name: binary_provenance(path.resolve(strict=True)) for name, path in sources.items()}
    except OSError as error:
        raise BenchError(f"required source provenance is missing: {error}") from error


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


def validate_clip_sidecar(sidecar: Any, expected_asset_id: str) -> dict[str, Any]:
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
    data = sidecar.get("data")
    if not isinstance(data, dict):
        raise BenchError(f"CLIP sidecar has no data object for {expected_asset_id}")
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
        or not isinstance(data.get("model"), str)
        or not data["model"]
        or data.get("embedding_dtype") != "float16"
        or data.get("embedding_encoding") != "base64"
        or data.get("frame_rate_sampled") != 0.5
        or not _finite_number(data.get("duration_s"))
        or data["duration_s"] <= 0
        or not isinstance(data.get("embeddings_b64"), str)
    ):
        raise BenchError(f"invalid CLIP data for {expected_asset_id}")
    try:
        encoded = base64.b64decode(data["embeddings_b64"], validate=True)
        values = memoryview(encoded).cast("e")
    except (TypeError, ValueError) as error:
        raise BenchError(f"invalid base64 float16 CLIP embeddings for {expected_asset_id}") from error
    if len(encoded) != frames * dimension * 2 or not all(math.isfinite(value) for value in values):
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


def validate_dispatcher_output(
    output_root: Path, run_label: str, expected_asset_ids: list[str]
) -> tuple[list[dict[str, Any]], dict[str, Any]]:
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
    records = [validate_clip_sidecar(read_json(sidecars[asset_id]), asset_id) for asset_id in expected_asset_ids]
    return records, {
        "dispatcher_report_path": str(report_path),
        "dispatcher_report_sha256": sha256_file(report_path),
        "dispatcher_summary": {key: report[key] for key in ("pair_count", "wrote", "skipped", "failed", "dep_skipped")},
        "retained_sidecars": {asset_id: str(sidecars[asset_id]) for asset_id in expected_asset_ids},
    }


def controlled_environment(
    sample_root: Path, python_root: Path, model_home: Path, uv: Path, ffmpeg: Path, ffprobe: Path
) -> dict[str, str]:
    environment = base_controlled_environment(sample_root, ffmpeg, ffprobe, uv)
    environment.pop("HF_TOKEN", None)
    environment["PATH"] = os.pathsep.join((str(python_root / ".venv/bin"), environment["PATH"]))
    environment.update(
        {
            "HOME": str(model_home),
            "MONTAGE_PYTHON_ROOT": str(python_root),
            "HF_HUB_OFFLINE": "1",
            "TRANSFORMERS_OFFLINE": "1",
            "CLIP_SAMPLE_FPS": "0.5",
            "PYTHONHASHSEED": "0",
            "LC_ALL": "C",
            "TZ": "UTC",
            "UV_OFFLINE": "1",
            "UV_NO_SYNC": "1",
            "UV_PROJECT_ENVIRONMENT": str(python_root / ".venv"),
        }
    )
    return environment


def run_sample(
    *,
    name: str,
    session_root: Path,
    binary: Path,
    assets: list[Path],
    python_root: Path,
    model_home: Path,
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
    observed_pids: set[int] = set()
    raw_samples: list[dict[str, Any]] = []
    peak_rss_bytes, started = 0, time.perf_counter()
    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        try:
            process = subprocess.Popen(
                command,
                cwd=ROOT,
                env=controlled_environment(sample_root, python_root, model_home, uv, ffmpeg, ffprobe),
                stdin=subprocess.DEVNULL,
                stdout=stdout,
                stderr=stderr,
                start_new_session=True,
            )
        except OSError as error:
            raise BenchError(f"spawn dispatcher: {error}") from error
        try:
            while process.poll() is None:
                sampled_at = time.perf_counter()
                rows = sample_processes()
                pids, rss_bytes = aggregate_process_tree(process.pid, rows)
                if any(pid == process.pid for pid, _, _ in rows):
                    observed_pids.update(pids)
                    peak_rss_bytes = max(peak_rss_bytes, rss_bytes)
                    raw_samples.append({
                        "elapsed_ms": (sampled_at - started) * 1000.0,
                        "pids": sorted(pids), "process_count": len(pids), "rss_bytes": rss_bytes,
                    })
                if sampled_at - started > timeout_seconds:
                    raise BenchError(f"{name} exceeded timeout of {timeout_seconds:.1f}s")
                time.sleep(max(0.0, SAMPLE_INTERVAL_SECONDS - (time.perf_counter() - sampled_at)))
        except BaseException:
            terminate_process_group(process)
            raise
    wall_seconds = time.perf_counter() - started
    if process.returncode != 0:
        tail = stderr_path.read_text(encoding="utf-8", errors="replace")[-4000:]
        raise BenchError(f"{name} dispatcher exited {process.returncode}: {tail}")
    if peak_rss_bytes <= 0:
        raise BenchError(f"{name} process-tree sampler did not observe positive RSS")
    orphans = wait_for_no_orphans(observed_pids - {process.pid})
    if orphans:
        raise BenchError(f"{name} cleanup failed: orphan_pids={orphans}")
    records, dispatcher = validate_dispatcher_output(output_root, name.replace("-", "_"), requested_asset_ids(assets))
    semantics, digest = canonical_data([record["stable_semantic_metadata"] for record in records])
    return {
        "name": name,
        "command": command,
        "wall_ms": wall_seconds * 1000.0,
        "process_tree_peak_rss_bytes": peak_rss_bytes,
        "sampler": sampler_evidence(name, wall_seconds, raw_samples),
        "cleanup": {"orphan_pids": [], "passed": True, "retained_work_root": str(work_root)},
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
    if args.samples < 5 or args.timeout_seconds <= 0:
        parser.error("--samples must be at least 5 and --timeout-seconds must be positive")
    if not LABEL_RE.fullmatch(args.label):
        parser.error("--label must contain only letters, digits, '-' or '_'")
    args.work_root = args.work_root.expanduser().resolve()
    args.evidence_dir = args.evidence_dir.expanduser().resolve() if args.evidence_dir else args.work_root / "evidence"
    return args


def run_benchmark(args: argparse.Namespace) -> Path:
    assets = validate_assets(args.asset)
    git = git_provenance()
    binary, python_root = resolve_executable(args.binary), resolve_python_workspace(args.python_root)
    weights, model_home, model = model_provenance(args.model_weights)
    uv = resolve_executable(os.environ.get("MONTAGE_UV", "uv"))
    ffmpeg = resolve_executable(os.environ.get("MONTAGE_FFMPEG", "ffmpeg"))
    ffprobe = resolve_executable(os.environ.get("MONTAGE_FFPROBE", "ffprobe"))
    args.work_root.mkdir(parents=True, exist_ok=True)
    args.evidence_dir.mkdir(parents=True, exist_ok=True)
    session_id = f"{args.label}-{dt.datetime.now(dt.UTC).strftime('%Y%m%dT%H%M%S')}-{os.getpid()}-{time.time_ns()}"
    session_root = args.work_root / "runs" / session_id
    session_root.mkdir(parents=True, exist_ok=False)
    common = {
        "session_root": session_root, "binary": binary, "assets": assets, "python_root": python_root,
        "model_home": model_home, "uv": uv, "ffmpeg": ffmpeg, "ffprobe": ffprobe,
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
    report = {
        "schema_version": 1,
        "generated_at_utc": dt.datetime.now(dt.UTC).isoformat(),
        "configuration": {
            "label": args.label, "warmups": 1, "samples": args.samples, "asset_count": ASSET_COUNT,
            "dispatcher_indexers": ["clip"], "dispatcher_concurrency": 2,
            "sample_interval_ms": SAMPLE_INTERVAL_SECONDS * 1000.0,
        },
        "fixtures": [
            {"asset_id": asset_id, **binary_provenance(asset)}
            for asset, asset_id in zip(assets, requested_asset_ids(assets), strict=True)
        ],
        "provenance": {
            "dispatcher_binary": binary_provenance(binary),
            "python_runtime": {
                "workspace": str(python_root),
                "executable": binary_provenance(resolve_executable(python_root / ".venv/bin/python")),
                "version": run_text([str(python_root / ".venv/bin/python"), "-c", "import sys; print(sys.version)"]).strip(),
            },
            "model": model,
            "tools": {"uv": tool_provenance(uv, ["--version"]), "ffmpeg": tool_provenance(ffmpeg, ["-version"]), "ffprobe": tool_provenance(ffprobe, ["-version"])},
            "sources": source_provenance(python_root),
            "git": git,
            "filesystems": {"work": filesystem_provenance(args.work_root), "evidence": filesystem_provenance(args.evidence_dir), "fixture": filesystem_provenance(assets[0]), "model": filesystem_provenance(weights)},
            "machine": {"system": platform.system(), "release": platform.release(), "machine": platform.machine(), "cpu_count": os.cpu_count()},
            "controlled_environment": {"MONTAGE_PYTHON_ROOT": str(python_root), "HF_HUB_OFFLINE": "1", "TRANSFORMERS_OFFLINE": "1", "CLIP_SAMPLE_FPS": "0.5", "PYTHONHASHSEED": "0", "LC_ALL": "C", "TZ": "UTC", "UV_OFFLINE": "1", "UV_NO_SYNC": "1", "HOME": str(model_home)},
        },
        "correctness": {
            "expected_asset_ids": requested_asset_ids(assets),
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
