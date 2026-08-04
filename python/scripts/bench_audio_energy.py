#!/usr/bin/env python3
"""Benchmark real audio-energy indexing through the prebuilt Rust dispatcher."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import math
import os
import platform
import re
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_DURATION_SECONDS = 60 * 60
DEFAULT_SAMPLES = 5
SAMPLE_INTERVAL_SECONDS = 0.025
MAX_SAMPLE_GAP_SECONDS = 0.250
ORPHAN_GRACE_SECONDS = 3.0
LABEL_RE = re.compile(r"^[A-Za-z0-9_-]+$")


class BenchError(RuntimeError):
    pass


def parse_ps_table(text: str) -> list[tuple[int, int, int]]:
    rows: list[tuple[int, int, int]] = []
    for line in text.splitlines():
        fields = line.split()
        if len(fields) < 3:
            continue
        try:
            rows.append((int(fields[0]), int(fields[1]), int(fields[2])))
        except ValueError:
            continue
    return rows


def aggregate_process_tree(
    root_pid: int, rows: list[tuple[int, int, int]]
) -> tuple[set[int], int]:
    by_pid = {pid: (ppid, rss_kib) for pid, ppid, rss_kib in rows}
    pids = {root_pid}
    while True:
        added = {pid for pid, (ppid, _) in by_pid.items() if ppid in pids} - pids
        if not added:
            break
        pids.update(added)
    rss_bytes = sum(by_pid[pid][1] * 1024 for pid in pids if pid in by_pid)
    return pids, rss_bytes


def parse_df_posix(text: str) -> tuple[str, str]:
    lines = [line for line in text.splitlines() if line.strip()]
    if len(lines) < 2:
        raise BenchError("df output did not contain a filesystem row")
    fields = lines[-1].split(maxsplit=5)
    if len(fields) != 6:
        raise BenchError(f"could not parse df filesystem row: {lines[-1]!r}")
    return fields[0], fields[5]


def summarize(values: list[float]) -> dict[str, float]:
    if not values:
        raise BenchError("cannot summarize an empty sample set")
    ordered = sorted(values)
    median = float(statistics.median(ordered))
    deviations = [abs(value - median) for value in ordered]
    p95_index = max(0, math.ceil(len(ordered) * 0.95) - 1)
    return {
        "median": median,
        "p95": float(ordered[p95_index]),
        "mad": float(statistics.median(deviations)),
        "min": float(ordered[0]),
        "max": float(ordered[-1]),
    }


def canonical_data(data: Any) -> tuple[bytes, str]:
    encoded = json.dumps(
        data,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
        allow_nan=False,
    ).encode("utf-8")
    return encoded, hashlib.sha256(encoded).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while block := handle.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def run_text(command: list[str], *, timeout: float = 30.0) -> str:
    try:
        completed = subprocess.run(
            command,
            check=True,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise BenchError(f"command failed: {command!r}: {error}") from error
    return completed.stdout


def resolve_executable(value: str | Path) -> Path:
    raw = os.fspath(value)
    candidate = Path(raw).expanduser() if os.sep in raw else None
    resolved = candidate if candidate is not None else Path(shutil.which(raw) or raw)
    try:
        resolved = resolved.resolve(strict=True)
    except OSError as error:
        raise BenchError(f"executable not found: {raw}: {error}") from error
    if not resolved.is_file() or not os.access(resolved, os.X_OK):
        raise BenchError(f"not an executable file: {resolved}")
    return resolved


def tool_provenance(path: Path, version_args: list[str]) -> dict[str, Any]:
    version = run_text([str(path), *version_args]).splitlines()
    if not version or not version[0].strip():
        raise BenchError(f"tool returned no version: {path}")
    stat = path.stat()
    return {
        "path": str(path),
        "version": version[0].strip(),
        "sha256": sha256_file(path),
        "size_bytes": stat.st_size,
        "mtime_ns": stat.st_mtime_ns,
    }


def binary_provenance(path: Path) -> dict[str, Any]:
    stat = path.stat()
    return {
        "path": str(path),
        "sha256": sha256_file(path),
        "size_bytes": stat.st_size,
        "mtime_ns": stat.st_mtime_ns,
    }


def parse_audio_runtime_provenance(raw: str, expected_module: Path) -> dict[str, Any]:
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as error:
        raise BenchError(f"audio-energy runtime returned invalid JSON: {error}") from error
    required = ("python", "executable", "module", "numpy", "scipy", "pyloudnorm")
    if not isinstance(value, dict) or any(
        not isinstance(value.get(key), str) or not value[key]
        for key in required
    ):
        raise BenchError(f"audio-energy runtime provenance is incomplete: {value!r}")

    executable = resolve_executable(value["executable"])
    try:
        module = Path(value["module"]).resolve(strict=True)
        expected = expected_module.resolve(strict=True)
    except OSError as error:
        raise BenchError(f"could not resolve audio-energy module provenance: {error}") from error
    if module != expected:
        raise BenchError(f"audio-energy resolved from {module}, expected {expected}")
    module_stat = module.stat()
    return {
        "python_version": value["python"],
        "executable": binary_provenance(executable),
        "module": {
            "path": str(module),
            "sha256": sha256_file(module),
            "size_bytes": module_stat.st_size,
        },
        "packages": {
            "numpy": value["numpy"],
            "scipy": value["scipy"],
            "pyloudnorm": value["pyloudnorm"],
        },
    }


def audio_runtime_provenance(uv: Path) -> dict[str, Any]:
    script = """
import importlib.metadata
import json
import sys

import audio_energy_mcp
import numpy
import scipy

print(json.dumps({
    "python": sys.version,
    "executable": sys.executable,
    "module": audio_energy_mcp.__file__,
    "numpy": numpy.__version__,
    "scipy": scipy.__version__,
    "pyloudnorm": importlib.metadata.version("pyloudnorm"),
}))
"""
    output = run_text(
        [
            str(uv),
            "--directory",
            str(ROOT / "python"),
            "run",
            "--frozen",
            "--package",
            "audio-energy-mcp",
            "python",
            "-c",
            script,
        ],
        timeout=120.0,
    )
    expected_module = (
        ROOT / "python/packages/audio-energy-mcp/src/audio_energy_mcp/__init__.py"
    )
    return parse_audio_runtime_provenance(output, expected_module)


def filesystem_provenance(path: Path) -> dict[str, Any]:
    output = run_text(["df", "-P", str(path)])
    device, mount = parse_df_posix(output)
    if sys.platform == "darwin":
        diskutil = resolve_executable(shutil.which("diskutil") or "/usr/sbin/diskutil")
        info = run_text([str(diskutil), "info", mount])
        fs_type = next(
            (
                line.split(":", 1)[1].strip()
                for line in info.splitlines()
                if line.strip().startswith("File System Personality:")
            ),
            "",
        )
    else:
        fs_type = run_text(["stat", "-f", "-c", "%T", mount]).strip()
    if not device or not mount or not fs_type:
        raise BenchError(f"incomplete filesystem provenance for {path}")
    return {
        "device": device,
        "mount": mount,
        "filesystem_type": fs_type,
        "st_dev": path.stat().st_dev,
    }


def atomic_write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.parent / f".{path.name}.{os.getpid()}.{time.time_ns()}.tmp"
    try:
        with temporary.open("x", encoding="utf-8") as handle:
            json.dump(value, handle, indent=2, sort_keys=True, allow_nan=False)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def fixture_generator_args(duration_seconds: int) -> list[str]:
    duration = str(duration_seconds)
    return [
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        f"sine=frequency=173:sample_rate=48000:duration={duration}",
        "-f",
        "lavfi",
        "-i",
        f"sine=frequency=997:sample_rate=48000:duration={duration}",
        "-f",
        "lavfi",
        "-i",
        f"anoisesrc=color=pink:sample_rate=48000:duration={duration}:seed=4242",
        "-filter_complex",
        "[0:a]volume=0.20[a0];[1:a]volume=0.12[a1];[2:a]volume=0.04[a2];"
        "[a0][a1][a2]amix=inputs=3:normalize=0,aformat=channel_layouts=stereo[a]",
        "-map",
        "[a]",
        "-map_metadata",
        "-1",
        "-c:a",
        "aac",
        "-b:a",
        "128k",
        "-threads",
        "1",
        "-movflags",
        "+faststart",
        "-metadata",
        "creation_time=1970-01-01T00:00:00Z",
        "-f",
        "ipod",
    ]


def probe_fixture(path: Path, ffprobe: Path, duration_seconds: int) -> dict[str, Any]:
    raw = run_text(
        [
            str(ffprobe),
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name,sample_rate,channels",
            "-show_entries",
            "format=duration,format_name",
            "-of",
            "json",
            str(path),
        ]
    )
    try:
        probe = json.loads(raw)
        streams = probe["streams"]
        duration = float(probe["format"]["duration"])
    except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        raise BenchError(f"invalid fixture probe for {path}: {error}") from error
    if not streams or streams[0].get("codec_name") != "aac" or duration <= 0:
        raise BenchError(f"fixture has no valid AAC audio stream: {probe!r}")
    if abs(duration - duration_seconds) > max(0.5, duration_seconds * 0.001):
        raise BenchError(
            f"fixture duration {duration:.6f}s differs from requested {duration_seconds}s"
        )
    return probe


def prepare_fixture(
    fixture_root: Path,
    duration_seconds: int,
    ffmpeg: Path,
    ffprobe: Path,
) -> dict[str, Any]:
    fixture_root.mkdir(parents=True, exist_ok=True)
    fixture = fixture_root / f"audio-energy-mixed-{duration_seconds}s.m4a"
    metadata_path = fixture_root / f"audio-energy-mixed-{duration_seconds}s.json"
    generator_args = fixture_generator_args(duration_seconds)
    generated = False

    if fixture.exists() or metadata_path.exists():
        if not fixture.is_file() or not metadata_path.is_file():
            raise BenchError("fixture and fixture metadata must either both exist or both be absent")
        try:
            metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise BenchError(f"read fixture metadata: {error}") from error
        if (
            metadata.get("schema_version") != 1
            or metadata.get("duration_seconds") != duration_seconds
            or metadata.get("generator_args") != generator_args
        ):
            raise BenchError(f"fixture metadata does not match requested fixture: {metadata_path}")
        if metadata.get("sha256") != sha256_file(fixture):
            raise BenchError(f"fixture checksum does not match metadata: {fixture}")
    else:
        generated = True
        partial = fixture_root / f".{fixture.name}.{os.getpid()}.{time.time_ns()}.tmp"
        generator_tool = tool_provenance(ffmpeg, ["-version"])
        try:
            subprocess.run(
                [str(ffmpeg), *generator_args, str(partial)],
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                timeout=max(120, min(1800, duration_seconds)),
            )
            os.replace(partial, fixture)
        except (OSError, subprocess.SubprocessError) as error:
            raise BenchError(f"fixture generation failed: {error}") from error
        finally:
            partial.unlink(missing_ok=True)
        metadata = {
            "schema_version": 1,
            "duration_seconds": duration_seconds,
            "generator_args": generator_args,
            "generator_argv_template": [
                str(ffmpeg),
                *generator_args,
                "<atomic-output>",
            ],
            "generator_tool": generator_tool,
            "sha256": sha256_file(fixture),
        }
        atomic_write_json(metadata_path, metadata)

    probe = probe_fixture(fixture, ffprobe, duration_seconds)
    stat = fixture.stat()
    return {
        "path": str(fixture),
        "metadata_path": str(metadata_path),
        "generated": generated,
        "duration_seconds": duration_seconds,
        "size_bytes": stat.st_size,
        "sha256": sha256_file(fixture),
        "generator_args": metadata["generator_args"],
        "generator_argv_template": metadata["generator_argv_template"],
        "generator_tool": metadata["generator_tool"],
        "probe": probe,
        "filesystem": filesystem_provenance(fixture),
    }


def sample_processes() -> list[tuple[int, int, int]]:
    try:
        completed = subprocess.run(
            ["ps", "-axo", "pid=,ppid=,rss="],
            check=True,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise BenchError(f"process-tree sampler failed: {error}") from error
    rows = parse_ps_table(completed.stdout)
    if not rows:
        raise BenchError("process-tree sampler returned no parseable rows")
    return rows


def directory_bytes(root: Path) -> int:
    if not root.exists():
        return 0
    total = 0
    for path in root.rglob("*"):
        try:
            if path.is_file() and not path.is_symlink():
                total += path.stat().st_size
        except FileNotFoundError:
            continue
        except OSError as error:
            raise BenchError(f"sample temp directory {root}: {error}") from error
    return total


def find_leaked_audio_temp_files(root: Path) -> list[str]:
    if not root.exists():
        return []
    return sorted(
        str(path)
        for path in root.rglob("montage-audio-*.f32")
        if path.is_file() or path.is_symlink()
    )


def validate_sample_observations(
    name: str,
    *,
    sampler_count: int,
    peak_rss_bytes: int,
    temp_high_water_bytes: int,
) -> None:
    if sampler_count <= 0 or peak_rss_bytes <= 0 or temp_high_water_bytes < 0:
        raise BenchError(
            f"{name} sampler incomplete: samples={sampler_count} rss={peak_rss_bytes} "
            f"temp={temp_high_water_bytes}"
        )


def pid_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def wait_for_no_orphans(pids: set[int]) -> list[int]:
    deadline = time.monotonic() + ORPHAN_GRACE_SECONDS
    alive = sorted(pid for pid in pids if pid_alive(pid))
    while alive and time.monotonic() < deadline:
        time.sleep(0.05)
        alive = sorted(pid for pid in alive if pid_alive(pid))
    return alive


def terminate_process_group(process: subprocess.Popen[Any]) -> None:
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=3)
    except (ProcessLookupError, subprocess.TimeoutExpired):
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=3)


def read_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise BenchError(f"read JSON {path}: {error}") from error


def validate_dispatcher_output(
    output_root: Path,
    run_label: str,
    fixture: dict[str, Any],
) -> tuple[bytes, str, dict[str, Any]]:
    dispatcher_report_path = output_root / f"{run_label}-indexing-performance.json"
    dispatcher_report = read_json(dispatcher_report_path)
    try:
        command = dispatcher_report["command"]
        report = dispatcher_report["report"]
        pairs = report["pairs"]
    except (KeyError, TypeError) as error:
        raise BenchError(f"malformed dispatcher report: {dispatcher_report_path}") from error
    if (
        command.get("included_indexers") != ["audio-energy"]
        or report.get("pair_count") != 1
        or report.get("wrote") != 1
        or report.get("failed") != 0
        or report.get("dep_skipped") != 0
        or len(pairs) != 1
        or pairs[0].get("indexer") != "audio-energy"
        or pairs[0].get("outcome") != "wrote"
    ):
        raise BenchError(f"dispatcher did not complete one audio-energy write: {report!r}")

    run_dirs = sorted(output_root.glob("index-run-*"))
    sidecars = [
        path
        for run_dir in run_dirs
        for path in run_dir.glob("audio-energy/**/*.json")
        if path.is_file()
    ]
    if len(run_dirs) != 1 or len(sidecars) != 1:
        raise BenchError(
            f"expected one copied audio-energy sidecar, got runs={run_dirs!r} sidecars={sidecars!r}"
        )
    sidecar = read_json(sidecars[0])
    if sidecar.get("indexer") != "audio-energy" or not isinstance(sidecar.get("data"), dict):
        raise BenchError(f"invalid audio-energy sidecar header: {sidecars[0]}")
    data = sidecar["data"]
    windows = data.get("windows")
    duration = data.get("duration_s")
    if (
        not isinstance(windows, list)
        or not windows
        or not isinstance(duration, (int, float))
        or duration <= 0
        or abs(float(duration) - fixture["duration_seconds"])
        > max(0.5, fixture["duration_seconds"] * 0.001)
        or data.get("sample_rate") != 48_000
        or not isinstance(data.get("true_peak_dbfs"), (int, float))
        or not isinstance(data.get("loudness_integrated_lufs"), (int, float))
    ):
        raise BenchError(f"audio-energy sidecar is empty/no-audio or malformed: {sidecars[0]}")
    canonical, digest = canonical_data(data)
    pair = pairs[0]
    metrics = {
        "dispatcher_report_path": str(dispatcher_report_path),
        "sidecar_path": str(sidecars[0]),
        "sidecar_size_bytes": sidecars[0].stat().st_size,
        "windows_count": len(windows),
        "duration_s": float(duration),
        "dispatcher_tool_ms": pair.get("tool_ms"),
        "dispatcher_total_ms": pair.get("total_ms"),
        "dispatcher_peak_rss_bytes": pair.get("peak_rss_bytes"),
    }
    if not all(isinstance(metrics[key], int) for key in ["dispatcher_tool_ms", "dispatcher_total_ms"]):
        raise BenchError(f"dispatcher timing provenance is incomplete: {pair!r}")
    return canonical, digest, metrics


def controlled_environment(sample_root: Path, ffmpeg: Path, ffprobe: Path, uv: Path) -> dict[str, str]:
    temp_root = sample_root / "work" / "tmp"
    config_root = sample_root / "config-home"
    temp_root.mkdir(parents=True)
    config_root.mkdir(parents=True)
    path_parts = [str(uv.parent), str(ffprobe.parent), str(ffmpeg.parent)]
    inherited_path = os.environ.get("PATH")
    if inherited_path:
        path_parts.append(inherited_path)
    environment = os.environ.copy()
    environment.update(
        {
            "PATH": os.pathsep.join(path_parts),
            "MONTAGE_PYTHON_ROOT": str(ROOT / "python"),
            "MONTAGE_FFMPEG": str(ffmpeg),
            "TMPDIR": str(temp_root),
            "TEMP": str(temp_root),
            "TMP": str(temp_root),
            "XDG_CONFIG_HOME": str(config_root),
            "PYTHONHASHSEED": "0",
            "LC_ALL": "C",
            "TZ": "UTC",
        }
    )
    return environment


def run_sample(
    *,
    name: str,
    session_root: Path,
    binary: Path,
    fixture: dict[str, Any],
    ffmpeg: Path,
    ffprobe: Path,
    uv: Path,
    timeout_seconds: float,
) -> tuple[dict[str, Any], bytes]:
    sample_root = session_root / name
    sample_root.mkdir(parents=True, exist_ok=False)
    work_root = sample_root / "work"
    output_root = sample_root / "output"
    output_root.mkdir()
    environment = controlled_environment(sample_root, ffmpeg, ffprobe, uv)
    run_label = name.replace("-", "_")
    command = [
        str(binary),
        "--asset",
        fixture["path"],
        "--output-dir",
        str(output_root),
        "--work-dir",
        str(work_root),
        "--label",
        run_label,
        "--concurrency",
        "1",
        "--indexers",
        "audio-energy",
    ]
    stdout_path = sample_root / "stdout.log"
    stderr_path = sample_root / "stderr.log"
    observed_pids: set[int] = set()
    peak_rss_bytes = 0
    temp_high_water_bytes = 0
    sampler_count = 0
    max_sample_gap_seconds = 0.0
    last_sample_started: float | None = None
    started = time.perf_counter()
    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        try:
            process = subprocess.Popen(
                command,
                cwd=ROOT,
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=stdout,
                stderr=stderr,
                start_new_session=True,
            )
        except OSError as error:
            raise BenchError(f"spawn dispatcher: {error}") from error
        try:
            while True:
                sample_started = time.perf_counter()
                if last_sample_started is not None:
                    max_sample_gap_seconds = max(
                        max_sample_gap_seconds, sample_started - last_sample_started
                    )
                last_sample_started = sample_started
                rows = sample_processes()
                tree_pids, rss_bytes = aggregate_process_tree(process.pid, rows)
                if process.pid in {pid for pid, _, _ in rows}:
                    sampler_count += 1
                    observed_pids.update(tree_pids)
                    peak_rss_bytes = max(peak_rss_bytes, rss_bytes)
                temp_high_water_bytes = max(
                    temp_high_water_bytes, directory_bytes(work_root / "tmp")
                )
                if process.poll() is not None:
                    break
                if sample_started - started > timeout_seconds:
                    raise BenchError(f"{name} exceeded timeout of {timeout_seconds:.1f}s")
                elapsed = time.perf_counter() - sample_started
                if elapsed < SAMPLE_INTERVAL_SECONDS:
                    time.sleep(SAMPLE_INTERVAL_SECONDS - elapsed)
        except BaseException:
            terminate_process_group(process)
            raise
    wall_seconds = time.perf_counter() - started

    if process.returncode != 0:
        stderr_tail = stderr_path.read_text(encoding="utf-8", errors="replace")[-4000:]
        raise BenchError(f"{name} dispatcher exited {process.returncode}: {stderr_tail}")
    validate_sample_observations(
        name,
        sampler_count=sampler_count,
        peak_rss_bytes=peak_rss_bytes,
        temp_high_water_bytes=temp_high_water_bytes,
    )
    if max_sample_gap_seconds > MAX_SAMPLE_GAP_SECONDS:
        raise BenchError(
            f"{name} sampler gap {max_sample_gap_seconds * 1000:.2f}ms exceeded "
            f"{MAX_SAMPLE_GAP_SECONDS * 1000:.0f}ms"
        )
    orphans = wait_for_no_orphans(observed_pids - {process.pid})
    remaining_temp_files = sorted(
        str(path)
        for path in (work_root / "tmp").rglob("*")
        if path.is_file() or path.is_symlink()
    )
    leaked_audio_temp_files = find_leaked_audio_temp_files(work_root / "tmp")
    if orphans or leaked_audio_temp_files:
        raise BenchError(
            f"{name} cleanup failed: orphans={orphans} "
            f"audio_temp_files={leaked_audio_temp_files}"
        )

    canonical, digest, output_metrics = validate_dispatcher_output(
        output_root, run_label, fixture
    )
    result = {
        "name": name,
        "command": command,
        "pid": process.pid,
        "wall_ms": wall_seconds * 1000.0,
        "process_tree_peak_rss_bytes": peak_rss_bytes,
        "temp_directory_high_water_bytes": temp_high_water_bytes,
        "sampler": {
            "target_interval_ms": SAMPLE_INTERVAL_SECONDS * 1000.0,
            "max_allowed_gap_ms": MAX_SAMPLE_GAP_SECONDS * 1000.0,
            "max_observed_gap_ms": max_sample_gap_seconds * 1000.0,
            "samples": sampler_count,
        },
        "cleanup": {
            "orphan_pids": [],
            "leaked_audio_temp_files": [],
            "remaining_temp_files": remaining_temp_files,
            "passed": True,
        },
        "canonical_data_sha256": digest,
        **output_metrics,
    }
    return result, canonical


def source_provenance() -> dict[str, dict[str, Any]]:
    sources = {
        "harness": Path(__file__).resolve(),
        "dispatcher_binary_source": ROOT / "crates/index/src/bin/montage-index-perf.rs",
        "dispatcher_source": ROOT / "crates/index/src/lib.rs",
        "audio_energy_source": ROOT
        / "python/packages/audio-energy-mcp/src/audio_energy_mcp/__init__.py",
        "indexer_defaults_source": ROOT / "crates/config/src/defaults.rs",
        "python_lock": ROOT / "python/uv.lock",
    }
    result: dict[str, dict[str, Any]] = {}
    for name, path in sources.items():
        if not path.is_file():
            raise BenchError(f"required source provenance file missing: {path}")
        result[name] = {
            "path": str(path),
            "sha256": sha256_file(path),
            "size_bytes": path.stat().st_size,
        }
    return result


def git_provenance() -> dict[str, Any]:
    head = run_text(["git", "-C", str(ROOT), "rev-parse", "HEAD"]).strip()
    status = run_text(["git", "-C", str(ROOT), "status", "--porcelain"])
    if not head:
        raise BenchError("git returned an empty HEAD")
    return {"head": head, "dirty": bool(status.strip()), "status_porcelain": status.rstrip()}


def find_uv(binary: Path) -> Path:
    sibling = binary.parent / ("uv.exe" if os.name == "nt" else "uv")
    if sibling.is_file():
        return resolve_executable(sibling)
    found = shutil.which("uv")
    if found:
        return resolve_executable(found)
    fallback = Path.home() / ".local/bin/uv"
    return resolve_executable(fallback)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        default=os.environ.get(
            "MONTAGE_INDEX_PERF_BINARY", str(ROOT / "target/release/montage-index-perf")
        ),
        help="prebuilt montage-index-perf executable",
    )
    parser.add_argument("--ffmpeg", default=os.environ.get("MONTAGE_FFMPEG", "ffmpeg"))
    parser.add_argument("--ffprobe", default=os.environ.get("MONTAGE_FFPROBE", "ffprobe"))
    parser.add_argument("--duration-seconds", type=int, default=DEFAULT_DURATION_SECONDS)
    parser.add_argument("--samples", type=int, default=DEFAULT_SAMPLES)
    parser.add_argument("--timeout-seconds", type=float, default=900.0)
    parser.add_argument(
        "--work-root",
        type=Path,
        default=Path(tempfile.gettempdir()) / "montage-audio-energy-perf",
    )
    parser.add_argument("--evidence-dir", type=Path)
    parser.add_argument("--label", default="audio-energy")
    args = parser.parse_args(argv)
    if args.duration_seconds <= 0:
        parser.error("--duration-seconds must be positive")
    if args.samples < 5:
        parser.error("--samples must be at least 5")
    if args.timeout_seconds <= 0:
        parser.error("--timeout-seconds must be positive")
    if not LABEL_RE.fullmatch(args.label):
        parser.error("--label must contain only letters, digits, '-' or '_'")
    args.work_root = args.work_root.expanduser().resolve()
    args.evidence_dir = (
        args.evidence_dir.expanduser().resolve()
        if args.evidence_dir
        else args.work_root / "evidence"
    )
    return args


def run_benchmark(args: argparse.Namespace) -> Path:
    binary = resolve_executable(args.binary)
    ffmpeg = resolve_executable(args.ffmpeg)
    ffprobe = resolve_executable(args.ffprobe)
    uv = find_uv(binary)
    audio_runtime = audio_runtime_provenance(uv)
    python_root = ROOT / "python"
    if not (python_root / "pyproject.toml").is_file():
        raise BenchError(f"python workspace missing: {python_root}")

    args.work_root.mkdir(parents=True, exist_ok=True)
    args.evidence_dir.mkdir(parents=True, exist_ok=True)
    session_id = (
        f"{args.label}-{dt.datetime.now(dt.UTC).strftime('%Y%m%dT%H%M%S')}-"
        f"{os.getpid()}-{time.time_ns()}"
    )
    session_root = args.work_root / "runs" / session_id
    session_root.mkdir(parents=True, exist_ok=False)
    fixture = prepare_fixture(
        args.work_root / "fixtures", args.duration_seconds, ffmpeg, ffprobe
    )

    common = {
        "session_root": session_root,
        "binary": binary,
        "fixture": fixture,
        "ffmpeg": ffmpeg,
        "ffprobe": ffprobe,
        "uv": uv,
        "timeout_seconds": args.timeout_seconds,
    }
    warmup, baseline_data = run_sample(name="warmup-00", **common)
    samples: list[dict[str, Any]] = []
    for index in range(1, args.samples + 1):
        sample, sample_data = run_sample(name=f"sample-{index:02d}", **common)
        if sample_data != baseline_data:
            raise BenchError(
                f"sample-{index:02d} audio-energy data differs from warmup: "
                f"{sample['canonical_data_sha256']} != {warmup['canonical_data_sha256']}"
            )
        samples.append(sample)

    report = {
        "schema_version": 1,
        "generated_at_utc": dt.datetime.now(dt.UTC).isoformat(),
        "configuration": {
            "label": args.label,
            "duration_seconds": args.duration_seconds,
            "warmups": 1,
            "samples": args.samples,
            "dispatcher_indexers": ["audio-energy"],
            "dispatcher_concurrency": 1,
            "work_root": str(args.work_root),
            "evidence_dir": str(args.evidence_dir),
        },
        "fixture": fixture,
        "provenance": {
            "dispatcher_binary": binary_provenance(binary),
            "audio_energy_runtime": audio_runtime,
            "tools": {
                "ffmpeg": tool_provenance(ffmpeg, ["-version"]),
                "ffprobe": tool_provenance(ffprobe, ["-version"]),
                "uv": tool_provenance(uv, ["--version"]),
            },
            "sources": source_provenance(),
            "git": git_provenance(),
            "filesystems": {
                "work": filesystem_provenance(args.work_root),
                "evidence": filesystem_provenance(args.evidence_dir),
            },
            "machine": {
                "system": platform.system(),
                "release": platform.release(),
                "machine": platform.machine(),
                "python": platform.python_version(),
                "cpu_count": os.cpu_count(),
            },
            "controlled_environment": {
                "MONTAGE_PYTHON_ROOT": str(python_root),
                "MONTAGE_FFMPEG": str(ffmpeg),
                "XDG_CONFIG_HOME": "isolated per sample",
                "TMPDIR": "isolated per sample",
                "UV_CACHE_DIR": "assigned by dispatcher under each sample work root",
                "PYTHONHASHSEED": "0",
                "LC_ALL": "C",
                "TZ": "UTC",
            },
        },
        "correctness": {
            "canonical_audio_energy_data_sha256": warmup["canonical_data_sha256"],
            "all_timed_samples_exactly_equal_to_warmup": True,
            "audio_stream_verified": True,
            "nonempty_audio_energy_verified": True,
        },
        "warmup": warmup,
        "samples": samples,
        "statistics": {
            "wall_ms": summarize([sample["wall_ms"] for sample in samples]),
            "process_tree_peak_rss_bytes": summarize(
                [float(sample["process_tree_peak_rss_bytes"]) for sample in samples]
            ),
            "temp_directory_high_water_bytes": summarize(
                [float(sample["temp_directory_high_water_bytes"]) for sample in samples]
            ),
            "dispatcher_tool_ms": summarize(
                [float(sample["dispatcher_tool_ms"]) for sample in samples]
            ),
        },
        "cleanup": {
            "all_samples_passed": all(sample["cleanup"]["passed"] for sample in samples),
            "session_roots_retained_for_evidence": True,
        },
    }
    output = args.evidence_dir / f"{session_id}-audio-energy-performance.json"
    if output.exists():
        raise BenchError(f"refusing to overwrite evidence: {output}")
    atomic_write_json(output, report)
    return output


def main(argv: list[str] | None = None) -> int:
    try:
        output = run_benchmark(parse_args(argv))
    except BenchError as error:
        print(f"bench-audio-energy: {error}", file=sys.stderr)
        return 1
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
