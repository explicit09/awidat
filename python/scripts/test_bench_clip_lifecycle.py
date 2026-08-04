from __future__ import annotations

import base64
import hashlib
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock


sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench_clip_lifecycle as bench


VALID_EMBEDDING_BYTES = b"\x00\x3c" + bytes(1022)
VALID_EMBEDDINGS_B64 = base64.b64encode(VALID_EMBEDDING_BYTES).decode("ascii")
NONFINITE_EMBEDDINGS_B64 = base64.b64encode(b"\x00\x7e" + bytes(1022)).decode("ascii")


def clip_sidecar(
    asset_id: str = "external/a.mp4",
    asset_sha256: str = "a" * 64,
    model: str = "ViT-B-32/openai",
    embedding_dim: int = 512,
    embeddings_b64: str = VALID_EMBEDDINGS_B64,
) -> dict[str, object]:
    return {
        "indexer": "clip",
        "indexer_version": "0.1.0",
        "schema_version": "1",
        "asset_id": asset_id,
        "asset_sha256": asset_sha256,
        "produced_at": "2026-08-04T12:00:00+00:00",
        "data": {
            "model": model,
            "embedding_dim": embedding_dim,
            "embedding_dtype": "float16",
            "embedding_encoding": "base64",
            "frame_rate_sampled": 0.5,
            "duration_s": 2.0,
            "frame_count": 1,
            "timestamps_s": [0.0],
            "embeddings_b64": embeddings_b64,
            "perf": {"model_load_ms": 12, "inference_ms": 3},
        },
    }


def runtime_preflight_fixture(
    directory: str,
) -> tuple[Path, dict[str, Path], Path, Path, str, dict[str, object]]:
    python_root = Path(directory) / "python"
    clip_module = python_root / "packages/clip-mcp/src/clip_mcp/__init__.py"
    montage_module = python_root / "packages/montage-mcp/src/montage_mcp/__init__.py"
    venv_root = python_root / ".venv"
    package_paths = {
        "clip-mcp": clip_module,
        "montage-mcp": montage_module,
        "open-clip-torch": venv_root / "lib/python3.11/site-packages/open_clip/__init__.py",
        "torch": venv_root / "lib/python3.11/site-packages/torch/__init__.py",
        "torchvision": venv_root / "lib/python3.11/site-packages/torchvision/__init__.py",
        "numpy": venv_root / "lib/python3.11/site-packages/numpy/__init__.py",
    }
    for path in package_paths.values():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("", encoding="utf-8")
    launcher = venv_root / "bin/python"
    launcher.parent.mkdir(parents=True, exist_ok=True)
    launcher.symlink_to(Path(sys.executable))
    weights = Path(directory) / "open_clip_model.safetensors"
    weights.write_bytes(b"offline model")
    digest = hashlib.sha256(b"offline model").hexdigest()
    payload: dict[str, object] = {
        "python": "3.11.0",
        "executable": str(launcher),
        "clip_model": "ViT-B-32/openai",
        "packages": {
            name: {"version": "1.0", "module_path": str(path)}
            for name, path in package_paths.items()
        },
        "pretrained": {
            "hf_hub": "timm/vit_base_patch32_clip_224.openai/",
            "resolved_path": str(weights),
            "sha256": digest,
        },
    }
    return python_root, package_paths, launcher, weights, digest, payload


class BenchClipLifecycleTests(unittest.TestCase):
    def test_clip_sidecar_decodes_little_endian_float16_and_hashes_its_bytes(self) -> None:
        record = bench.validate_clip_sidecar(
            clip_sidecar(), "external/a.mp4", "a" * 64
        )

        self.assertEqual(
            record["embedding_bytes_sha256"],
            "c8a731278eb691f7ae43fe8b1aca8f771544bddbbec63046e50970a73766e1d3",
        )

    def test_clip_sidecar_rejects_nonfinite_little_endian_float16_embeddings(self) -> None:
        with self.assertRaisesRegex(bench.BenchError, "invalid float16 CLIP embeddings"):
            bench.validate_clip_sidecar(
                clip_sidecar(embeddings_b64=NONFINITE_EMBEDDINGS_B64),
                "external/a.mp4",
                "a" * 64,
            )

    def test_clip_sidecar_rejects_float16_byte_length_mismatch(self) -> None:
        with self.assertRaisesRegex(bench.BenchError, "invalid float16 CLIP embeddings"):
            bench.validate_clip_sidecar(
                clip_sidecar(embeddings_b64="ADw="), "external/a.mp4", "a" * 64
            )

    def test_clip_sidecar_rejects_an_unpinned_model(self) -> None:
        with self.assertRaisesRegex(bench.BenchError, "pinned CLIP model"):
            bench.validate_clip_sidecar(
                clip_sidecar(model="ViT-L-14/laion2b_s32b_b82k"),
                "external/a.mp4",
                "a" * 64,
            )

    def test_clip_sidecar_rejects_the_wrong_embedding_dimension(self) -> None:
        with self.assertRaisesRegex(bench.BenchError, "512-dimensional"):
            bench.validate_clip_sidecar(
                clip_sidecar(embedding_dim=3, embeddings_b64="ADwAQABC"),
                "external/a.mp4",
                "a" * 64,
            )

    def test_controller_python_provenance_records_the_controller_runtime(self) -> None:
        provenance = bench.controller_python_provenance()
        executable = Path(sys.executable).resolve(strict=True)

        self.assertEqual(provenance["sys_executable"], sys.executable)
        self.assertEqual(provenance["sys_version"], sys.version)
        self.assertEqual(
            provenance["resolved_binary"],
            {
                "path": str(executable),
                "sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
                "size_bytes": executable.stat().st_size,
                "mtime_ns": executable.stat().st_mtime_ns,
            },
        )

    def test_git_provenance_rejects_an_untracked_python_startup_hook(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "python").mkdir()
            tracked = root / "python/tracked.py"
            tracked.write_text("VALUE = 1\n", encoding="utf-8")
            (root / ".gitignore").write_text(
                "python/sitecustomize.py\n", encoding="utf-8"
            )
            for command in (
                ["git", "init", "--quiet"],
                ["git", "config", "user.email", "benchmark@example.invalid"],
                ["git", "config", "user.name", "Benchmark Test"],
                ["git", "add", ".gitignore", "python/tracked.py"],
                ["git", "commit", "--quiet", "-m", "fixture"],
            ):
                subprocess.run(command, cwd=root, check=True, capture_output=True)
            (root / "python/sitecustomize.py").write_text(
                "raise RuntimeError('untracked startup hook executed')\n",
                encoding="utf-8",
            )

            with mock.patch.object(bench, "ROOT", root):
                with self.assertRaisesRegex(bench.BenchError, "dirty source state"):
                    bench.git_provenance()

    def test_source_provenance_binds_ignored_venv_file_contents(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            python_root = Path(directory)
            for relative_path in (
                "packages/clip-mcp/src/clip_mcp/__init__.py",
                "packages/montage-mcp/src/montage_mcp/__init__.py",
                "uv.lock",
            ):
                path = python_root / relative_path
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("fixture\n", encoding="utf-8")
            dependency = python_root / ".venv/lib/python3.11/site-packages/torch/kernel.py"
            dependency.parent.mkdir(parents=True)
            dependency.write_text("VALUE = 1\n", encoding="utf-8")
            original_stat = dependency.stat()

            before = bench.source_provenance(python_root)
            dependency.write_text("VALUE = 2\n", encoding="utf-8")
            os.utime(
                dependency,
                ns=(original_stat.st_atime_ns, original_stat.st_mtime_ns),
            )
            after = bench.source_provenance(python_root)

        self.assertNotEqual(before, after)

    def test_clip_sidecar_records_exact_float16_embedding_and_stable_metadata(self) -> None:
        record = bench.validate_clip_sidecar(clip_sidecar(), "external/a.mp4", "a" * 64)

        self.assertEqual(
            record,
            {
                "asset_id": "external/a.mp4",
                "timestamps_s": [0.0],
                "frame_count": 1,
                "model": "ViT-B-32/openai",
                "embedding_dtype": "float16",
                "embedding_encoding": "base64",
                "embedding_dim": 512,
                "embedding_bytes_sha256": (
                    "c8a731278eb691f7ae43fe8b1aca8f771544bddbbec63046e50970a73766e1d3"
                ),
                "stable_semantic_metadata": {
                    "indexer": "clip",
                    "indexer_version": "0.1.0",
                    "schema_version": "1",
                    "asset_id": "external/a.mp4",
                    "asset_sha256": "a" * 64,
                    "model": "ViT-B-32/openai",
                    "embedding_dim": 512,
                    "embedding_dtype": "float16",
                    "embedding_encoding": "base64",
                    "frame_rate_sampled": 0.5,
                    "duration_s": 2.0,
                    "frame_count": 1,
                    "timestamps_s": [0.0],
                    "embedding_bytes_sha256": (
                        "c8a731278eb691f7ae43fe8b1aca8f771544bddbbec63046e50970a73766e1d3"
                    ),
                },
            },
        )

    def test_dispatcher_output_rejects_malformed_or_partial_six_asset_results(self) -> None:
        expected_ids = [f"external/{index}.mp4" for index in range(6)]
        fingerprints = {asset_id: "a" * 64 for asset_id in expected_ids}
        with tempfile.TemporaryDirectory() as directory:
            output_root = Path(directory)
            malformed = output_root / "malformed-indexing-performance.json"
            malformed.write_text(json.dumps({"command": {}}), encoding="utf-8")
            with self.assertRaisesRegex(bench.BenchError, "malformed dispatcher report"):
                bench.validate_dispatcher_output(output_root, "malformed", expected_ids, fingerprints)

            partial = output_root / "partial-indexing-performance.json"
            partial.write_text(
                json.dumps(
                    {
                        "command": {
                            "included_indexers": ["clip"],
                            "concurrency": 2,
                            "assets": expected_ids,
                        },
                        "report": {
                            "pair_count": 5,
                            "wrote": 5,
                            "skipped": 0,
                            "failed": 0,
                            "dep_skipped": 0,
                            "pairs": [
                                {
                                    "indexer": "clip",
                                    "asset_id": asset_id,
                                    "outcome": "wrote",
                                }
                                for asset_id in expected_ids[:-1]
                            ],
                        },
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(bench.BenchError, "six CLIP writes"):
                bench.validate_dispatcher_output(output_root, "partial", expected_ids, fingerprints)

    def test_summary_uses_nearest_rank_p95_and_median_absolute_deviation(self) -> None:
        self.assertEqual(
            bench.summarize([10.0, 12.0, 11.0, 50.0, 13.0]),
            {"median": 12.0, "p95": 50.0, "mad": 1.0, "min": 10.0, "max": 50.0},
        )

    def test_sampler_reports_the_gap_between_adjacent_process_tree_samples(self) -> None:
        evidence = bench.sampler_evidence(
            "sample",
            0.1,
            [
                {"elapsed_ms": 0.0, "pids": [10], "process_count": 1, "rss_bytes": 1024},
                {"elapsed_ms": 25.0, "pids": [10, 11], "process_count": 2, "rss_bytes": 2048},
            ],
        )

        self.assertEqual(evidence["samples"], 2)
        self.assertEqual(evidence["gap_ms"]["max"], 25.0)

    def test_process_tree_rss_includes_descendants_but_not_unrelated_processes(self) -> None:
        pids, rss_bytes = bench.aggregate_process_tree(
            10,
            [(10, 1, 4), (11, 10, 8), (12, 11, 16), (13, 1, 32)],
        )

        self.assertEqual(pids, {10, 11, 12})
        self.assertEqual(rss_bytes, 28 * 1024)

    def test_model_provenance_derives_hf_home_without_replacing_home(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            hf_home = Path(directory) / "huggingface"
            blob = hf_home / "hub/models--timm--vit_base_patch32_clip_224.openai/blobs/test-blob"
            blob.parent.mkdir(parents=True)
            blob.write_bytes(b"pinned weights")
            snapshot = (
                hf_home
                / "hub/models--timm--vit_base_patch32_clip_224.openai/snapshots"
                / "a6f597a30f7b82c51704746581f9a4e41421e878/open_clip_model.safetensors"
            )
            snapshot.parent.mkdir(parents=True)
            snapshot.symlink_to("../../blobs/test-blob")
            digest = hashlib.sha256(b"pinned weights").hexdigest()

            with mock.patch.object(bench, "MODEL_SHA256", digest):
                weights, derived_hf_home, provenance = bench.model_provenance(snapshot)
            with mock.patch.dict(os.environ, {"HOME": "/preserve-this-home"}, clear=False):
                environment = bench.controlled_environment(
                    sample_root=Path(directory) / "sample",
                    python_root=Path(directory) / "python",
                    hf_home=derived_hf_home,
                    uv=Path(directory) / "uv",
                    ffmpeg=Path(directory) / "ffmpeg",
                    ffprobe=Path(directory) / "ffprobe",
                )

        self.assertEqual(weights, snapshot)
        self.assertEqual(derived_hf_home, hf_home)
        self.assertEqual(provenance["snapshot_path"], str(snapshot))
        self.assertEqual(provenance["resolved_blob"]["sha256"], digest)
        self.assertEqual(environment["HF_HOME"], str(hf_home))
        self.assertEqual(environment["HOME"], "/preserve-this-home")

    def test_controlled_environment_excludes_external_python_code_paths(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with mock.patch.dict(
                os.environ, {"PYTHONPATH": "/untrusted/python"}, clear=False
            ):
                environment = bench.controlled_environment(
                    sample_root=Path(directory) / "sample",
                    python_root=Path(directory) / "python",
                    hf_home=Path(directory) / "huggingface",
                    uv=Path(directory) / "uv",
                    ffmpeg=Path(directory) / "ffmpeg",
                    ffprobe=Path(directory) / "ffprobe",
                )

        self.assertNotIn("PYTHONPATH", environment)
        self.assertEqual(environment["PYTHONNOUSERSITE"], "1")
        self.assertEqual(environment["PYTHONDONTWRITEBYTECODE"], "1")
        self.assertEqual(environment["PYTHONSAFEPATH"], "1")

    def test_asset_fingerprint_matches_the_rust_metadata_algorithm_and_binds_sidecar(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            asset = Path(directory) / "clip-fixture.mp4"
            asset.write_bytes(b"clip fixture")
            os.utime(asset, ns=(1_700_000_000_123_456_789, 1_700_000_000_123_456_789))

            fingerprint = bench.asset_fingerprint(asset)
            self.assertEqual(
                fingerprint,
                "b5c8849787067a0ee5c049eb456f7dfbe7a0e8bfd0680b1cbe8b3b7c8165f289",
            )
            bench.validate_clip_sidecar(clip_sidecar(asset_sha256=fingerprint), "external/a.mp4", fingerprint)
            with self.assertRaisesRegex(bench.BenchError, "asset fingerprint"):
                bench.validate_clip_sidecar(clip_sidecar(), "external/a.mp4", fingerprint)
            pre_epoch = Path(directory) / "pre-epoch.mp4"
            pre_epoch.write_bytes(b"x")
            os.utime(pre_epoch, ns=(-1, -1))
            self.assertEqual(
                bench.asset_fingerprint(pre_epoch),
                "6225f9979a3c51a55215b48867e90d1628058db1d913e2cc9fa4441825a285b3",
            )

    def test_runtime_preflight_rejects_a_dependency_resolved_outside_the_supplied_venv(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            python_root, package_paths, launcher, weights, digest, payload = (
                runtime_preflight_fixture(directory)
            )
            self.assertEqual(bench.python_launcher(python_root), launcher)
            with mock.patch.object(bench, "MODEL_SHA256", digest):
                result = bench.parse_runtime_preflight(
                    json.dumps(payload), python_root, weights
                )
                self.assertEqual(
                    result["packages"]["clip-mcp"]["path"],
                    str(package_paths["clip-mcp"].resolve()),
                )
                self.assertEqual(result["offline_artifact_resolver"]["sha256"], digest)
                outside = Path(directory) / "outside-torch.py"
                outside.write_text("", encoding="utf-8")
                payload["packages"]["torch"]["module_path"] = str(outside)
                with self.assertRaisesRegex(bench.BenchError, "outside supplied .venv"):
                    bench.parse_runtime_preflight(
                        json.dumps(payload), python_root, weights
                    )

    def test_runtime_preflight_rejects_an_unpinned_clip_module_model(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            python_root, _package_paths, _launcher, weights, digest, payload = (
                runtime_preflight_fixture(directory)
            )
            payload["clip_model"] = "ViT-L-14/laion2b_s32b_b82k"

            with mock.patch.object(bench, "MODEL_SHA256", digest):
                with self.assertRaisesRegex(bench.BenchError, "pinned CLIP model"):
                    bench.parse_runtime_preflight(
                        json.dumps(payload), python_root, weights
                    )

    def test_process_group_cleanup_polls_and_terminates_only_the_dispatcher_group(self) -> None:
        process = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(60)"], start_new_session=True
        )
        try:
            members: list[tuple[int, int, int, int]] = []
            for _ in range(50):
                members = bench.process_group_members(process.pid)
                if any(member[0] == process.pid for member in members):
                    break
                time.sleep(0.02)
            self.assertIn(process.pid, {member[0] for member in members})
            self.assertEqual(bench.terminate_process_group(process.pid, process), [])
            self.assertIsNotNone(process.wait(timeout=3))
            self.assertEqual(bench.process_group_members(process.pid), [])
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=3)

    def test_successful_dispatcher_exit_rejects_a_worker_requiring_forced_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pid_path = root / "dispatcher.pid"
            binary = root / "leaky-dispatcher"
            binary.write_text(
                f"#!{sys.executable}\n"
                "import os\n"
                "import subprocess\n"
                "import time\n"
                "from pathlib import Path\n"
                f"Path({str(pid_path)!r}).write_text(str(os.getpid()), encoding='utf-8')\n"
                "subprocess.Popen(['/bin/sleep', '60'])\n"
                "time.sleep(0.1)\n",
                encoding="utf-8",
            )
            binary.chmod(0o755)
            session_root = root / "session"
            session_root.mkdir()
            assets = [root / f"asset-{index}.mp4" for index in range(bench.ASSET_COUNT)]
            fingerprints = {
                asset_id: "a" * 64
                for asset_id in bench.requested_asset_ids(assets)
            }

            try:
                with mock.patch.object(
                    bench, "validate_dispatcher_output", return_value=([], {})
                ), mock.patch.object(bench, "sampler_evidence", return_value={}):
                    with self.assertRaisesRegex(bench.BenchError, "required forced cleanup"):
                        bench.run_sample(
                            name="leaky-success",
                            session_root=session_root,
                            binary=binary,
                            assets=assets,
                            asset_fingerprints=fingerprints,
                            python_root=root / "python",
                            hf_home=root / "huggingface",
                            uv=Path("/usr/bin/true"),
                            ffmpeg=Path("/usr/bin/true"),
                            ffprobe=Path("/usr/bin/true"),
                            timeout_seconds=5.0,
                        )
                pgid = int(pid_path.read_text(encoding="utf-8"))
                self.assertEqual(bench.process_group_members(pgid), [])
            finally:
                if pid_path.exists():
                    bench.terminate_process_group(
                        int(pid_path.read_text(encoding="utf-8"))
                    )

    def test_dispatcher_output_retains_pair_and_sidecar_timing_attribution(self) -> None:
        expected_ids = [f"external/{index}.mp4" for index in range(6)]
        fingerprints = {asset_id: f"{index:064x}" for index, asset_id in enumerate(expected_ids)}
        with tempfile.TemporaryDirectory() as directory:
            output_root = Path(directory)
            pairs = [
                {
                    "indexer": "clip",
                    "asset_id": asset_id,
                    "outcome": "wrote",
                    "queued_ms": 1,
                    "launch_init_ms": 2,
                    "tool_ms": 3,
                    "write_ms": 4,
                    "total_ms": 10,
                    "peak_rss_bytes": 4096,
                    "sidecar": {
                        "perf": {"model_load_ms": 5, "inference_ms": 6, "frames_processed": 1}
                    },
                }
                for asset_id in expected_ids
            ]
            (output_root / "complete-indexing-performance.json").write_text(
                json.dumps(
                    {
                        "command": {
                            "included_indexers": ["clip"],
                            "concurrency": 2,
                            "assets": expected_ids,
                        },
                        "report": {
                            "pair_count": 6,
                            "wrote": 6,
                            "skipped": 0,
                            "failed": 0,
                            "dep_skipped": 0,
                            "pairs": pairs,
                        },
                    }
                ),
                encoding="utf-8",
            )
            for asset_id in expected_ids:
                sidecar = output_root / "index-run-test/clip" / f"{asset_id}.json"
                sidecar.parent.mkdir(parents=True, exist_ok=True)
                sidecar.write_text(
                    json.dumps(clip_sidecar(asset_id, fingerprints[asset_id])), encoding="utf-8"
                )

            _records, dispatcher = bench.validate_dispatcher_output(
                output_root, "complete", expected_ids, fingerprints
            )

        self.assertEqual(
            dispatcher["pair_telemetry"][0],
            {
                "asset_id": "external/0.mp4",
                "queued_ms": 1,
                "launch_init_ms": 2,
                "tool_ms": 3,
                "write_ms": 4,
                "total_ms": 10,
                "direct_peak_rss_bytes": 4096,
                "sidecar_perf": {"model_load_ms": 5, "inference_ms": 6, "frames_processed": 1},
            },
        )


if __name__ == "__main__":
    unittest.main()
