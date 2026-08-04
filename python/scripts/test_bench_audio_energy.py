from __future__ import annotations

from contextlib import redirect_stderr
import io
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench_audio_energy as bench


def audio_energy_data() -> dict[str, object]:
    return {
        "sample_rate": 48_000,
        "duration_s": 2.0,
        "window_ms": 100,
        "windows": [
            {"start_s": index / 10.0, "rms_db": -12.0}
            for index in range(20)
        ],
        "loudness_integrated_lufs": -18.0,
        "true_peak_dbfs": -1.0,
        "loudness_short_term": [{"start_s": 0.0, "lufs": -18.0}],
        "silences": [{"start_s": 0.5, "end_s": 1.0}],
        "silence_relative_lu": -30.0,
    }


def write_dispatcher_output(
    output_root: Path,
    data: dict[str, object],
    *,
    pair_overrides: dict[str, object] | None = None,
) -> None:
    pair: dict[str, object] = {
        "indexer": "audio-energy",
        "outcome": "wrote",
        "tool_ms": 10,
        "total_ms": 12,
        "peak_rss_bytes": 4096,
    }
    pair.update(pair_overrides or {})
    (output_root / "sample-indexing-performance.json").write_text(
        json.dumps(
            {
                "command": {"included_indexers": ["audio-energy"]},
                "report": {
                    "pair_count": 1,
                    "wrote": 1,
                    "failed": 0,
                    "dep_skipped": 0,
                    "pairs": [pair],
                },
            }
        ),
        encoding="utf-8",
    )
    sidecar = output_root / "index-run-test/audio-energy/external/fixture.m4a.json"
    sidecar.parent.mkdir(parents=True)
    sidecar.write_text(
        json.dumps({"indexer": "audio-energy", "data": data}),
        encoding="utf-8",
    )


def generate_fake_fixture(command: list[str], **_kwargs: object) -> None:
    Path(command[-1]).write_bytes(b"deterministic fixture")


class BenchAudioEnergyTests(unittest.TestCase):
    def run_mocked_benchmark(
        self,
        root: Path,
        *,
        input_states: list[dict[str, object]],
        write: mock.Mock,
    ) -> Path:
        binary = root / "montage-index-perf"
        ffmpeg = root / "ffmpeg"
        ffprobe = root / "ffprobe"
        uv = root / "uv"
        fixture_path = root / "fixture.m4a"
        metadata_path = root / "fixture.json"
        fixture_path.write_bytes(b"fixture")
        metadata_path.write_text("{}", encoding="utf-8")
        fixture = {
            "path": str(fixture_path),
            "metadata_path": str(metadata_path),
            "duration_seconds": 2,
        }
        sample = {
            "canonical_data_sha256": "stable",
            "wall_ms": 10.0,
            "process_tree_peak_rss_bytes": 4096,
            "temp_directory_high_water_bytes": 0,
            "dispatcher_tool_ms": 8,
            "cleanup": {"passed": True},
        }
        args = SimpleNamespace(
            binary=binary,
            ffmpeg=ffmpeg,
            ffprobe=ffprobe,
            duration_seconds=2,
            samples=1,
            timeout_seconds=60.0,
            work_root=root / "work",
            evidence_dir=root / "evidence",
            label="test",
        )
        with (
            mock.patch.object(
                bench, "resolve_executable", side_effect=lambda value: Path(value)
            ),
            mock.patch.object(bench, "find_uv", return_value=uv),
            mock.patch.object(bench, "audio_runtime_provenance", return_value={}),
            mock.patch.object(bench, "prepare_fixture", return_value=fixture),
            mock.patch.object(
                bench,
                "execution_inputs_provenance",
                side_effect=input_states,
                create=True,
            ),
            mock.patch.object(
                bench, "run_sample", return_value=(sample, b"stable")
            ),
            mock.patch.object(
                bench, "temp_directory_observation", return_value={}
            ),
            mock.patch.object(bench, "binary_provenance", return_value={}),
            mock.patch.object(bench, "tool_provenance", return_value={}),
            mock.patch.object(bench, "source_provenance", return_value={}),
            mock.patch.object(bench, "git_provenance", return_value={}),
            mock.patch.object(
                bench,
                "filesystem_provenance",
                side_effect=lambda path: {"path": str(path)},
            ),
            mock.patch.object(bench, "atomic_write_json", write),
        ):
            return bench.run_benchmark(args)

    def test_process_table_parser_ignores_headers_and_malformed_rows(self) -> None:
        rows = bench.parse_ps_table(
            " PID PPID RSS STAT\n"
            " 41 1 1024 S\n"
            " 42 41 2048 Z\n"
            " missing fields\n"
            " 43 42 nope S\n"
        )

        self.assertEqual(rows, [(41, 1, 1024, "S"), (42, 41, 2048, "Z")])

    def test_recursive_tree_rss_excludes_unrelated_processes(self) -> None:
        pids, rss_bytes = bench.aggregate_process_tree(
            10,
            [
                (10, 1, 4),
                (11, 10, 8),
                (12, 11, 16),
                (13, 1, 32),
            ],
        )

        self.assertEqual(pids, {10, 11, 12})
        self.assertEqual(rss_bytes, 28 * 1024)

    def test_process_snapshot_identifies_an_unreaped_zombie_leader(self) -> None:
        rows = [(41, 1, 1024, "Z"), (42, 41, 2048, "S")]

        self.assertTrue(bench.dispatcher_exited_in_snapshot(41, rows))
        self.assertFalse(bench.dispatcher_exited_in_snapshot(42, rows))

    def test_process_sampler_collects_state_in_its_single_snapshot(self) -> None:
        completed = SimpleNamespace(stdout="41 1 1024 Z\n")
        with mock.patch.object(bench.subprocess, "run", return_value=completed) as run:
            rows = bench.sample_processes()

        self.assertEqual(rows, [(41, 1, 1024, "Z")])
        run.assert_called_once_with(
            ["ps", "-axo", "pid=,ppid=,rss=,stat="],
            check=True,
            capture_output=True,
            text=True,
            timeout=5,
        )

    def test_df_parser_preserves_mount_paths_with_spaces(self) -> None:
        device, mount = bench.parse_df_posix(
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n"
            "/dev/disk9s1 1000 400 600 40% /Volumes/My Passport for Mac\n"
        )

        self.assertEqual(device, "/dev/disk9s1")
        self.assertEqual(mount, "/Volumes/My Passport for Mac")

    def test_summary_uses_nearest_rank_p95_and_median_absolute_deviation(self) -> None:
        self.assertEqual(
            bench.summarize([10.0, 12.0, 11.0, 50.0, 13.0]),
            {
                "median": 12.0,
                "p95": 50.0,
                "mad": 1.0,
                "min": 10.0,
                "max": 50.0,
            },
        )

    def test_canonical_data_hash_is_stable_across_mapping_order(self) -> None:
        canonical, digest = bench.canonical_data({"b": [2, 1], "a": 1.25})

        self.assertEqual(canonical, b'{"a":1.25,"b":[2,1]}')
        self.assertEqual(
            digest,
            "843dc42a984a1c7f307f2d0b00159e385a1db0b7edf56989dcc00590fb8126fc",
        )

    def test_canonical_data_reports_nonfinite_json_as_a_benchmark_error(self) -> None:
        with self.assertRaisesRegex(bench.BenchError, "canonical JSON"):
            bench.canonical_data({"value": float("nan")})

    def test_benchmark_rejects_execution_inputs_changed_during_samples(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            write = mock.Mock()
            with self.assertRaisesRegex(bench.BenchError, "execution inputs changed"):
                self.run_mocked_benchmark(
                    Path(directory),
                    input_states=[
                        {"dispatcher_binary": {"sha256": "a" * 64}},
                        {"dispatcher_binary": {"sha256": "b" * 64}},
                    ],
                    write=write,
                )

        write.assert_not_called()

    def test_workspace_manifest_binds_same_stat_venv_content_changes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            python_root = Path(directory)
            dependency = python_root / ".venv/lib/python3.11/site-packages/numpy/core.py"
            dependency.parent.mkdir(parents=True)
            dependency.write_text("VALUE = 1\n", encoding="utf-8")
            original_stat = dependency.stat()

            before = bench.workspace_manifest(python_root)
            dependency.write_text("VALUE = 2\n", encoding="utf-8")
            os.utime(
                dependency,
                ns=(original_stat.st_atime_ns, original_stat.st_mtime_ns),
            )
            after = bench.workspace_manifest(python_root)

        self.assertNotEqual(before, after)

    def test_benchmark_output_locations_reject_a_runtime_workspace_descendant(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory) / "python"
            workspace.mkdir()

            with self.assertRaisesRegex(
                bench.BenchError, "inside the Python runtime workspace"
            ):
                bench.validate_benchmark_output_locations(
                    workspace,
                    {"work root": workspace / "benchmark-output"},
                )

    def test_benchmark_rejects_a_repository_output_root_before_fixture_preparation(
        self,
    ) -> None:
        work_root = bench.ROOT / ".audio-energy-benchmark-test-output"
        args = SimpleNamespace(
            binary=Path("/fake/montage-index-perf"),
            ffmpeg=Path("/fake/ffmpeg"),
            ffprobe=Path("/fake/ffprobe"),
            duration_seconds=2,
            samples=5,
            timeout_seconds=60.0,
            work_root=work_root,
            evidence_dir=work_root / "evidence",
            label="test",
        )
        prepare_fixture = mock.Mock()

        with (
            mock.patch.object(
                bench, "resolve_executable", side_effect=lambda value: Path(value)
            ),
            mock.patch.object(bench, "find_uv", return_value=Path("/fake/uv")),
            mock.patch.object(bench, "prepare_fixture", prepare_fixture),
            self.assertRaisesRegex(bench.BenchError, "inside repository root"),
        ):
            bench.run_benchmark(args)

        prepare_fixture.assert_not_called()

    def test_controlled_environment_rejects_a_shadowed_provenanced_tool(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            uv = root / "uv/bin/uv"
            ffmpeg = root / "ffmpeg/bin/ffmpeg"
            ffprobe = root / "ffprobe/bin/ffprobe"
            shadow_ffmpeg = uv.parent / "ffmpeg"
            for executable in (uv, ffmpeg, ffprobe, shadow_ffmpeg):
                executable.parent.mkdir(parents=True, exist_ok=True)
                executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
                executable.chmod(0o755)

            with self.assertRaisesRegex(bench.BenchError, "PATH resolves ffmpeg"):
                bench.controlled_environment(
                    root / "sample", ffmpeg, ffprobe, uv
                )

    def test_fixture_cache_lock_blocks_another_process_until_release(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            lock_path = Path(directory) / "fixture.lock"
            script = (
                "import fcntl,sys\n"
                "handle=open(sys.argv[1], 'a+b')\n"
                "fcntl.flock(handle.fileno(), fcntl.LOCK_EX)\n"
                "print('acquired', flush=True)\n"
            )
            with bench.fixture_cache_lock(lock_path):
                process = subprocess.Popen(
                    [sys.executable, "-c", script, str(lock_path)],
                    stdout=subprocess.PIPE,
                    text=True,
                )
                time.sleep(0.1)
                self.assertIsNone(process.poll())
            stdout, _stderr = process.communicate(timeout=3)

        self.assertEqual(process.returncode, 0)
        self.assertEqual(stdout.strip(), "acquired")

    def test_fixture_cache_recovers_either_lone_publication(self) -> None:
        for orphan in ("media", "metadata"):
            with self.subTest(orphan=orphan), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                fixture = root / "audio-energy-mixed-2s.m4a"
                metadata = root / "audio-energy-mixed-2s.json"
                if orphan == "media":
                    fixture.write_bytes(b"orphan")
                else:
                    metadata.write_text("{}", encoding="utf-8")

                with (
                    mock.patch.object(
                        bench.subprocess, "run", side_effect=generate_fake_fixture
                    ),
                    mock.patch.object(
                        bench, "tool_provenance", return_value={"path": "ffmpeg"}
                    ),
                    mock.patch.object(bench, "probe_fixture", return_value={"ok": True}),
                    mock.patch.object(
                        bench, "filesystem_provenance", return_value={"mount": "test"}
                    ),
                ):
                    result = bench.prepare_fixture(
                        root, 2, Path("/fake/ffmpeg"), Path("/fake/ffprobe")
                    )

                self.assertTrue(result["generated"])
                self.assertTrue(fixture.is_file())
                self.assertTrue(metadata.is_file())
                self.assertEqual(
                    json.loads(metadata.read_text(encoding="utf-8"))["sha256"],
                    bench.sha256_file(fixture),
                )

    def test_fixture_cache_still_rejects_a_corrupt_complete_pair(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = root / "audio-energy-mixed-2s.m4a"
            metadata = root / "audio-energy-mixed-2s.json"
            fixture.write_bytes(b"corrupt")
            generator_args = bench.fixture_generator_args(2)
            metadata.write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "duration_seconds": 2,
                        "generator_args": generator_args,
                        "generator_argv_template": [
                            "/fake/ffmpeg",
                            *generator_args,
                            "<atomic-output>",
                        ],
                        "generator_tool": {
                            "path": "/fake/ffmpeg",
                            "version": "fake ffmpeg",
                            "sha256": "a" * 64,
                            "size_bytes": 1,
                            "mtime_ns": 1,
                        },
                        "sha256": "0" * 64,
                    }
                ),
                encoding="utf-8",
            )
            with mock.patch.object(bench.subprocess, "run") as generate:
                with self.assertRaisesRegex(bench.BenchError, "checksum"):
                    bench.prepare_fixture(
                        root, 2, Path("/fake/ffmpeg"), Path("/fake/ffprobe")
                    )

        generate.assert_not_called()

    def test_fixture_cache_rejects_incomplete_metadata_as_a_benchmark_error(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = root / "audio-energy-mixed-2s.m4a"
            metadata = root / "audio-energy-mixed-2s.json"
            fixture.write_bytes(b"fixture")
            checksum = bench.sha256_file(fixture)
            cases: list[object] = [
                [],
                {
                    "schema_version": 1,
                    "duration_seconds": 2,
                    "generator_args": bench.fixture_generator_args(2),
                    "sha256": checksum,
                },
            ]
            for value in cases:
                with self.subTest(value=value):
                    metadata.write_text(json.dumps(value), encoding="utf-8")
                    with self.assertRaisesRegex(
                        bench.BenchError, "fixture metadata is incomplete"
                    ):
                        bench.prepare_fixture(
                            root, 2, Path("/fake/ffmpeg"), Path("/fake/ffprobe")
                        )

    def test_fixture_cache_recovers_after_metadata_publication_failure(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = root / "audio-energy-mixed-2s.m4a"
            metadata = root / "audio-energy-mixed-2s.json"
            with (
                mock.patch.object(
                    bench.subprocess, "run", side_effect=generate_fake_fixture
                ),
                mock.patch.object(
                    bench, "tool_provenance", return_value={"path": "ffmpeg"}
                ),
                mock.patch.object(bench, "probe_fixture", return_value={"ok": True}),
                mock.patch.object(
                    bench, "filesystem_provenance", return_value={"mount": "test"}
                ),
                mock.patch.object(
                    bench, "atomic_write_json", side_effect=OSError("disk full")
                ),
            ):
                with self.assertRaisesRegex(bench.BenchError, "metadata publication"):
                    bench.prepare_fixture(
                        root, 2, Path("/fake/ffmpeg"), Path("/fake/ffprobe")
                    )
            self.assertTrue(fixture.is_file())
            self.assertFalse(metadata.exists())

            with (
                mock.patch.object(
                    bench.subprocess, "run", side_effect=generate_fake_fixture
                ),
                mock.patch.object(
                    bench, "tool_provenance", return_value={"path": "ffmpeg"}
                ),
                mock.patch.object(bench, "probe_fixture", return_value={"ok": True}),
                mock.patch.object(
                    bench, "filesystem_provenance", return_value={"mount": "test"}
                ),
            ):
                bench.prepare_fixture(
                    root, 2, Path("/fake/ffmpeg"), Path("/fake/ffprobe")
                )

            self.assertTrue(fixture.is_file())
            self.assertTrue(metadata.is_file())

    def test_dispatcher_output_rejects_malformed_audio_numeric_fields(self) -> None:
        cases = {
            "boolean duration": lambda data: data.__setitem__("duration_s", True),
            "boolean peak": lambda data: data.__setitem__("true_peak_dbfs", True),
            "nonfinite loudness": lambda data: data.__setitem__(
                "loudness_integrated_lufs", float("inf")
            ),
            "boolean window size": lambda data: data.__setitem__("window_ms", True),
            "non-integer window size": lambda data: data.__setitem__(
                "window_ms", 100.0
            ),
            "non-object window": lambda data: data.__setitem__("windows", [True]),
            "nonfinite window value": lambda data: data.__setitem__(
                "windows", [{"start_s": 0.0, "rms_db": float("nan")}]
            ),
            "nonfinite short-term value": lambda data: data.__setitem__(
                "loudness_short_term",
                [{"start_s": 0.0, "lufs": float("inf")}],
            ),
            "nonfinite silence value": lambda data: data.__setitem__(
                "silences", [{"start_s": 0.5, "end_s": float("nan")}]
            ),
            "boolean silence threshold": lambda data: data.__setitem__(
                "silence_relative_lu", True
            ),
        }
        for name, mutate in cases.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                data = audio_energy_data()
                mutate(data)
                output_root = Path(directory)
                write_dispatcher_output(output_root, data)

                with self.assertRaises(bench.BenchError):
                    bench.validate_dispatcher_output(
                        output_root, "sample", {"duration_seconds": 2.0}
                    )

    def test_dispatcher_output_accepts_complete_rms_window_grid(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output_root = Path(directory)
            write_dispatcher_output(output_root, audio_energy_data())

            _, _, metrics = bench.validate_dispatcher_output(
                output_root, "sample", {"duration_seconds": 2.0}
            )

            self.assertEqual(metrics["windows_count"], 20)

    def test_dispatcher_output_requires_complete_rms_window_grid(self) -> None:
        cases = {
            "truncated windows": lambda data: data["windows"].pop(),
            "misaligned window": lambda data: data["windows"][1].__setitem__(
                "start_s", 0.15
            ),
        }
        for name, mutate in cases.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                data = audio_energy_data()
                mutate(data)
                output_root = Path(directory)
                write_dispatcher_output(output_root, data)

                with self.assertRaises(bench.BenchError):
                    bench.validate_dispatcher_output(
                        output_root, "sample", {"duration_seconds": 2.0}
                    )

    def test_dispatcher_output_rejects_invalid_timing_and_rss_provenance(self) -> None:
        cases = {
            "boolean tool time": {"tool_ms": True},
            "negative total time": {"total_ms": -1},
            "string peak RSS": {"peak_rss_bytes": "4096"},
            "negative peak RSS": {"peak_rss_bytes": -1},
        }
        for name, overrides in cases.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                output_root = Path(directory)
                write_dispatcher_output(
                    output_root, audio_energy_data(), pair_overrides=overrides
                )

                with self.assertRaises(bench.BenchError):
                    bench.validate_dispatcher_output(
                        output_root, "sample", {"duration_seconds": 2.0}
                    )

    def test_audio_temp_leak_check_ignores_uv_locks(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "uv-cache.lock").write_text("", encoding="utf-8")
            leaked = root / "montage-audio-leaked.f32"
            leaked.write_bytes(b"pcm")

            self.assertEqual(bench.find_leaked_audio_temp_files(root), [str(leaked)])

    def test_run_sample_terminates_an_observed_worker_after_dispatcher_exit(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            child_pid_path = root / "child.pid"
            binary = root / "leaky-dispatcher"
            binary.write_text(
                f"#!{sys.executable}\n"
                "import subprocess\n"
                "import time\n"
                "from pathlib import Path\n"
                "child = subprocess.Popen(['/bin/sleep', '60'])\n"
                f"Path({str(child_pid_path)!r}).write_text(str(child.pid), encoding='utf-8')\n"
                "time.sleep(0.5)\n",
                encoding="utf-8",
            )
            binary.chmod(0o755)
            tool_root = root / "tools"
            tool_root.mkdir()
            uv = tool_root / "uv"
            ffmpeg = tool_root / "ffmpeg"
            ffprobe = tool_root / "ffprobe"
            for executable in (uv, ffmpeg, ffprobe):
                executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
                executable.chmod(0o755)
            fixture = root / "fixture.m4a"
            fixture.write_bytes(b"fixture")
            session_root = root / "session"
            session_root.mkdir()
            child_pid: int | None = None

            try:
                with (
                    mock.patch.object(bench, "ORPHAN_GRACE_SECONDS", 0.1),
                    mock.patch.object(bench, "wait_for_no_orphans", return_value=[]),
                    mock.patch.object(
                        bench, "validate_sample_observations", return_value=None
                    ),
                    mock.patch.object(
                        bench,
                        "validate_sampler_timing",
                        return_value={"observed_rate_hz": 40.0, "gap_ms": {}},
                    ),
                ):
                    with self.assertRaisesRegex(
                        bench.BenchError, "required forced process-group cleanup"
                    ):
                        bench.run_sample(
                            name="leaky-success",
                            session_root=session_root,
                            binary=binary,
                            fixture={"path": str(fixture), "duration_seconds": 2.0},
                            ffmpeg=ffmpeg,
                            ffprobe=ffprobe,
                            uv=uv,
                            timeout_seconds=5.0,
                        )
                child_pid = int(child_pid_path.read_text(encoding="utf-8"))
                self.assertFalse(bench.pid_alive(child_pid))
            finally:
                if child_pid is None and child_pid_path.exists():
                    child_pid = int(child_pid_path.read_text(encoding="utf-8"))
                if child_pid is not None and bench.pid_alive(child_pid):
                    os.kill(child_pid, signal.SIGKILL)

    def test_cleanup_never_signals_a_reaped_dispatcher_process_group(self) -> None:
        process = mock.Mock(pid=12345, returncode=0)
        process.poll.return_value = 0

        with (
            mock.patch.object(bench, "wait_for_no_orphans", return_value=[]),
            mock.patch.object(bench.os, "killpg") as killpg,
        ):
            cleanup = bench.terminate_process_group(process, {67890})

        self.assertEqual(cleanup.remaining_group_members, [])
        self.assertEqual(cleanup.observed_remaining_members, [])
        killpg.assert_not_called()

    def test_cleanup_keeps_the_leader_unreaped_through_group_signals(self) -> None:
        class Process:
            pid = 12345
            returncode: int | None = None

            def poll(self) -> int | None:
                return self.returncode

            def wait(self, *, timeout: float) -> int:
                self.returncode = 0
                return 0

        process = Process()
        returncodes_at_signal: list[int | None] = []

        def record_group_signal(_pgid: int, _signal: signal.Signals) -> None:
            returncodes_at_signal.append(process.returncode)

        with (
            mock.patch.object(
                bench,
                "process_group_nonquiescent_members",
                return_value=[67890],
            ),
            mock.patch.object(
                bench,
                "wait_for_process_group_quiescence",
                side_effect=[[67890], []],
            ),
            mock.patch.object(
                bench,
                "wait_for_no_orphans",
                return_value=[],
            ),
            mock.patch.object(bench.os, "killpg", side_effect=record_group_signal),
        ):
            cleanup = bench.terminate_process_group(process, {67890})

        self.assertEqual(cleanup.remaining_group_members, [])
        self.assertEqual(returncodes_at_signal, [None, None])

    def test_cleanup_reports_forced_and_remaining_group_members(self) -> None:
        class Process:
            pid = 12345
            returncode: int | None = None

            def wait(self, *, timeout: float) -> int:
                self.returncode = 0
                return 0

        process = Process()
        with (
            mock.patch.object(
                bench,
                "process_group_nonquiescent_members",
                return_value=[67890],
            ),
            mock.patch.object(
                bench,
                "wait_for_process_group_quiescence",
                side_effect=[[67890], [67890]],
            ),
            mock.patch.object(bench, "wait_for_no_orphans", return_value=[]),
            mock.patch.object(bench.os, "killpg"),
        ):
            cleanup = bench.terminate_process_group(process, set())

        self.assertEqual(cleanup.forced_group_members, [67890])
        self.assertEqual(cleanup.remaining_group_members, [67890])

    def test_cleanup_falls_back_to_group_signals_when_sampling_fails(self) -> None:
        class Process:
            pid = 12345
            returncode: int | None = None

            def __init__(self) -> None:
                self.wait_timeouts: list[float] = []

            def wait(self, *, timeout: float) -> int:
                self.wait_timeouts.append(timeout)
                self.returncode = 0
                return 0

        process = Process()
        returncodes_at_signal: list[int | None] = []
        signals: list[signal.Signals] = []

        def record_group_signal(_pgid: int, group_signal: signal.Signals) -> None:
            returncodes_at_signal.append(process.returncode)
            signals.append(group_signal)

        with (
            mock.patch.object(
                bench,
                "process_group_nonquiescent_members",
                side_effect=bench.BenchError("initial group sample failed"),
            ),
            mock.patch.object(
                bench,
                "wait_for_process_group_quiescence",
                side_effect=[bench.BenchError("final group sample failed"), []],
            ),
            mock.patch.object(bench, "wait_for_no_orphans", return_value=[]),
            mock.patch.object(bench.os, "killpg", side_effect=record_group_signal),
        ):
            cleanup = bench.terminate_process_group(process, set())

        self.assertEqual(signals, [signal.SIGTERM, signal.SIGKILL])
        self.assertEqual(returncodes_at_signal, [None, None])
        self.assertEqual(process.wait_timeouts, [3])
        self.assertEqual(cleanup.forced_group_members, [])
        self.assertEqual(cleanup.remaining_group_members, [])
        self.assertEqual(
            cleanup.errors,
            ["initial group sample failed", "final group sample failed"],
        )

    def test_cleanup_kills_an_untracked_term_ignoring_group_descendant(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            child_pid_path = Path(directory) / "child.pid"
            process = subprocess.Popen(
                [
                    sys.executable,
                    "-c",
                    (
                        "import pathlib, subprocess, sys; "
                        "child = subprocess.Popen([sys.executable, '-c', "
                        "'import signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); "
                        "time.sleep(60)']); "
                        "pathlib.Path(sys.argv[1]).write_text(str(child.pid), encoding='utf-8')"
                    ),
                    str(child_pid_path),
                ],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                start_new_session=True,
            )
            child_pid: int | None = None

            try:
                deadline = time.monotonic() + 1.0
                while not child_pid_path.exists():
                    self.assertLess(time.monotonic(), deadline, "child PID was not recorded")
                    time.sleep(0.01)
                child_pid = int(child_pid_path.read_text(encoding="utf-8"))
                while not bench.dispatcher_exited_without_reaping(process):
                    self.assertLess(time.monotonic(), deadline, "leader did not exit")
                    time.sleep(0.01)

                with mock.patch.object(bench, "ORPHAN_GRACE_SECONDS", 0.1):
                    cleanup = bench.terminate_process_group(process, set())

                self.assertEqual(cleanup.remaining_group_members, [])
                self.assertEqual(cleanup.forced_group_members, [child_pid])

                self.assertFalse(bench.pid_alive(child_pid))
            finally:
                if child_pid is not None and bench.pid_alive(child_pid):
                    os.kill(child_pid, signal.SIGKILL)
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=3)

    def test_success_cleanup_allows_an_untracked_short_lived_group_descendant(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            child_pid_path = Path(directory) / "child.pid"
            process = subprocess.Popen(
                [
                    sys.executable,
                    "-c",
                    (
                        "import pathlib, subprocess, sys; "
                        "child = subprocess.Popen([sys.executable, '-c', "
                        "'import time; time.sleep(0.1)']); "
                        "pathlib.Path(sys.argv[1]).write_text(str(child.pid), encoding='utf-8')"
                    ),
                    str(child_pid_path),
                ],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                start_new_session=True,
            )
            child_pid: int | None = None

            try:
                deadline = time.monotonic() + 1.0
                while not child_pid_path.exists():
                    self.assertLess(time.monotonic(), deadline, "child PID was not recorded")
                    time.sleep(0.01)
                child_pid = int(child_pid_path.read_text(encoding="utf-8"))
                while not bench.dispatcher_exited_without_reaping(process):
                    self.assertLess(time.monotonic(), deadline, "leader did not exit")
                    time.sleep(0.01)

                with (
                    mock.patch.object(bench, "ORPHAN_GRACE_SECONDS", 0.5),
                    mock.patch.object(bench.os, "killpg") as killpg,
                ):
                    cleanup = bench.terminate_process_group(
                        process, set(), allow_natural_exit=True
                    )

                killpg.assert_not_called()
                self.assertEqual(cleanup.forced_group_members, [])
                self.assertEqual(cleanup.remaining_group_members, [])
                self.assertEqual(cleanup.errors, [])
                self.assertFalse(bench.pid_alive(child_pid))
            finally:
                if child_pid is not None and bench.pid_alive(child_pid):
                    os.kill(child_pid, signal.SIGKILL)
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=3)

    def test_dispatcher_exit_observer_does_not_require_waitid(self) -> None:
        process = subprocess.Popen(
            [sys.executable, "-c", ""],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        try:
            deadline = time.monotonic() + 1.0
            with mock.patch.object(bench.os, "waitid", new=None, create=True):
                while not bench.dispatcher_exited_without_reaping(process):
                    self.assertLess(time.monotonic(), deadline, "leader did not exit")
                    time.sleep(0.01)
            self.assertIsNone(process.returncode)
        finally:
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)

    def test_parse_args_rejects_nonfinite_timeout_values(self) -> None:
        with redirect_stderr(io.StringIO()):
            for value in ("nan", "inf", "-inf"):
                with self.subTest(value=value), self.assertRaises(SystemExit):
                    bench.parse_args([f"--timeout-seconds={value}"])

    def test_zero_temp_high_water_is_a_valid_streaming_result(self) -> None:
        bench.validate_sample_observations(
            "candidate",
            sampler_count=1,
            peak_rss_bytes=1024,
            temp_high_water_bytes=0,
        )

    def test_temp_directory_observation_includes_nonzero_warmup_baseline(self) -> None:
        observation = bench.temp_directory_observation(
            [
                {
                    "temp_directory_high_water_bytes": 4096,
                    "sampler": {
                        "gap_ms": {"max": 40.0},
                        "observed_rate_hz": 38.5,
                    },
                    "cleanup": {
                        "passed": True,
                        "leaked_audio_temp_files": [],
                        "remaining_temp_files": ["/tmp/uv-cache.lock"],
                    },
                },
                {
                    "temp_directory_high_water_bytes": 2048,
                    "sampler": {
                        "gap_ms": {"max": 55.0},
                        "observed_rate_hz": 20.0,
                    },
                    "cleanup": {
                        "passed": True,
                        "leaked_audio_temp_files": [],
                        "remaining_temp_files": [],
                    },
                },
            ]
        )

        self.assertEqual(
            observation,
            {
                "method": "periodic recursive byte-size polling of isolated TMPDIRs",
                "target_interval_ms": 25.0,
                "maximum_observed_high_water_bytes": 4096,
                "maximum_observed_sampler_gap_ms": 55.0,
                "minimum_observed_rate_hz": 20.0,
                "post_run_audio_temp_leak_check": {
                    "passed": True,
                    "leaked_audio_temp_files": [],
                },
                "limitation": (
                    "Periodic polling cannot exclude transient files between samples "
                    "or prove decoder transport."
                ),
            },
        )

    def test_audio_runtime_provenance_resolves_the_executed_environment(self) -> None:
        runtime = bench.parse_audio_runtime_provenance(
            json.dumps(
                {
                    "python": sys.version,
                    "executable": sys.executable,
                    "module": bench.__file__,
                    "numpy": "2.4.4",
                    "scipy": "1.17.1",
                    "pyloudnorm": "0.2.0",
                    "module_paths": {
                        "audio_energy_mcp": bench.__file__,
                        "numpy": bench.__file__,
                        "scipy": bench.__file__,
                        "pyloudnorm": bench.__file__,
                    },
                }
            ),
            Path(bench.__file__).resolve(),
        )

        self.assertEqual(runtime["python_version"], sys.version)
        self.assertEqual(runtime["module"]["path"], str(Path(bench.__file__).resolve()))
        self.assertEqual(
            runtime["packages"],
            {"numpy": "2.4.4", "scipy": "1.17.1", "pyloudnorm": "0.2.0"},
        )
        self.assertEqual(runtime["executable"]["path"], str(Path(sys.executable).resolve()))

    def test_python_runtime_environment_removes_hostile_uv_and_import_overrides(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            hostile = Path(directory)
            (hostile / "audio_energy_mcp.py").write_text("raise RuntimeError\n")
            inherited = {
                "PYTHONPATH": str(hostile),
                "PYTHONHOME": str(hostile),
                "PYTHONUSERBASE": str(hostile),
                "PYTHONSTARTUP": str(hostile / "startup.py"),
                "PYTHONWARNINGS": "error",
                "VIRTUAL_ENV": str(hostile),
                "UV_PROJECT": str(hostile),
                "UV_PYTHON": str(hostile / "python"),
                "UV_PROJECT_ENVIRONMENT": str(hostile / "venv"),
                "UV_CONFIG_FILE": str(hostile / "uv.toml"),
                "UV_OFFLINE": "0",
            }
            with mock.patch.dict(
                bench.os.environ,
                inherited,
                clear=True,
            ):
                environment = bench.python_runtime_environment()

        for key in inherited:
            if key in {"UV_OFFLINE", "UV_PROJECT_ENVIRONMENT"}:
                continue
            self.assertNotIn(key, environment)
        self.assertNotIn("VIRTUAL_ENV", environment)
        self.assertEqual(environment["PYTHONNOUSERSITE"], "1")
        self.assertEqual(environment["PYTHONDONTWRITEBYTECODE"], "1")
        self.assertEqual(environment["PYTHONHASHSEED"], "0")
        self.assertEqual(environment["PYTHONUTF8"], "1")
        self.assertEqual(environment["UV_OFFLINE"], "1")
        self.assertEqual(environment["UV_NO_SYNC"], "1")
        self.assertEqual(environment["UV_NO_CONFIG"], "1")
        self.assertEqual(environment["UV_PYTHON_DOWNLOADS"], "never")
        self.assertEqual(environment["LC_ALL"], "C")
        self.assertEqual(environment["LANG"], "C")
        self.assertEqual(environment["TZ"], "UTC")

    def test_preflight_and_sample_share_the_sanitized_python_environment(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            uv = root / "tools/uv"
            ffmpeg = root / "tools/ffmpeg"
            ffprobe = root / "tools/ffprobe"
            for executable in (uv, ffmpeg, ffprobe):
                executable.parent.mkdir(parents=True, exist_ok=True)
                executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
                executable.chmod(0o755)

            with mock.patch.dict(
                bench.os.environ,
                {
                    "PYTHONPATH": "/hostile",
                    "PYTHONUSERBASE": "/hostile",
                    "VIRTUAL_ENV": "/hostile",
                    "UV_PROJECT": "/hostile",
                    "UV_PYTHON": "/hostile/python",
                    "UV_PROJECT_ENVIRONMENT": "/hostile/venv",
                },
                clear=True,
            ):
                preflight = bench.python_runtime_environment()
                sample = bench.controlled_environment(root / "sample", ffmpeg, ffprobe, uv)

        for key in (
            "MONTAGE_PYTHON_ROOT",
            "PYTHONNOUSERSITE",
            "PYTHONDONTWRITEBYTECODE",
            "PYTHONSAFEPATH",
            "PYTHONHASHSEED",
            "PYTHONUTF8",
            "UV_OFFLINE",
            "UV_NO_SYNC",
            "UV_NO_CONFIG",
            "UV_PYTHON_DOWNLOADS",
            "UV_PROJECT_ENVIRONMENT",
            "LC_ALL",
            "LANG",
            "TZ",
        ):
            self.assertEqual(sample[key], preflight[key])
        self.assertNotIn("PYTHONPATH", sample)
        self.assertNotIn("PYTHONUSERBASE", sample)
        self.assertNotIn("VIRTUAL_ENV", sample)
        self.assertNotIn("UV_PROJECT", sample)
        self.assertNotIn("UV_PYTHON", sample)

    def test_find_uv_prefers_dispatcher_sibling_to_environment_override(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "bin/montage-index-perf"
            sibling = binary.parent / "uv"
            override = root / "override/uv"
            for executable in (binary, sibling, override):
                executable.parent.mkdir(parents=True, exist_ok=True)
                executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
                executable.chmod(0o755)

            with mock.patch.dict(
                bench.os.environ, {"MONTAGE_UV": str(override)}, clear=True
            ):
                resolved = bench.find_uv(binary)

        self.assertEqual(resolved, sibling.resolve())

    def test_runtime_provenance_rejects_dependency_outside_prepared_venv(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory) / "python"
            module = workspace / "packages/audio-energy-mcp/src/audio_energy_mcp/__init__.py"
            module.parent.mkdir(parents=True)
            module.write_text("", encoding="utf-8")
            runtime_python = workspace / ".venv/bin/python"
            runtime_python.parent.mkdir(parents=True)
            runtime_python.symlink_to(Path(sys.executable))
            venv_module = workspace / ".venv/lib/python3.11/site-packages/numpy/__init__.py"
            venv_module.parent.mkdir(parents=True)
            venv_module.write_text("", encoding="utf-8")
            hostile = Path(directory) / "hostile.py"
            hostile.write_text("", encoding="utf-8")
            raw = json.dumps(
                {
                    "python": sys.version,
                    "executable": sys.executable,
                    "module": str(module),
                    "numpy": "2.4.4",
                    "scipy": "1.17.1",
                    "pyloudnorm": "0.2.0",
                    "module_paths": {
                        "audio_energy_mcp": str(module),
                        "numpy": str(hostile),
                        "scipy": str(venv_module),
                        "pyloudnorm": str(venv_module),
                    },
                }
            )

            with self.assertRaisesRegex(bench.BenchError, "outside prepared venv"):
                bench.parse_audio_runtime_provenance(
                    raw, module, workspace_root=workspace
                )

    def test_sampler_timing_allows_a_recorded_swap_stall_with_dense_coverage(self) -> None:
        timing = bench.validate_sampler_timing(
            "baseline",
            wall_seconds=3.64,
            sampler_count=101,
            sample_gaps_seconds=[0.025] * 100 + [1.14],
        )

        self.assertAlmostEqual(timing["observed_rate_hz"], 101 / 3.64)
        self.assertEqual(timing["gap_ms"]["max"], 1140.0)

    def test_sampler_timing_rejects_sparse_coverage(self) -> None:
        with self.assertRaisesRegex(bench.BenchError, "sample rate"):
            bench.validate_sampler_timing(
                "baseline",
                wall_seconds=10.0,
                sampler_count=10,
                sample_gaps_seconds=[1.0] * 9,
            )


if __name__ == "__main__":
    unittest.main()
