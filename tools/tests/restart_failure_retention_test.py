"""Restart-only diagnostics: no Bus, provider, watcher or compilation starts here."""
import json
import os
from pathlib import Path
import re
import shutil
import tempfile
import time
import uuid

import pytest

from tools.acceptance import native_restart_test as restart


class FailedBus:
    def __init__(self, root, fail_reads=False):
        self.data = root / "data"
        self.fail_reads = fail_reads
        self.calls = []
        self.stopped = False

    def cli(self, *args, timeout):
        assert not self.stopped, "diagnostics must precede cleanup"
        self.calls.append(args)
        if self.fail_reads:
            raise OSError("control unavailable")
        if args == ("state",):
            return {"agents": [{"id": 7, "runtime_identity": {"pane_id": "w1:p2"}}]}
        if args[:2] == ("message", "status"):
            return {"message_id": args[2], "stage": "blocked"}
        assert args[:2] == ("agent", "observe")
        return {"text": "startup prompt", "terminal_id": "fixture-terminal"}

    def native_agent(self, pane):
        assert not self.stopped
        assert pane == "w1:p2"
        return {"pane_id": pane, "launch_pending": True}

    def stop(self):
        self.stopped = True
        (self.data / "state.json").write_text("changed by cleanup")


def test_snapshot_keeps_bootstrap_and_native_evidence_before_cleanup(tmp_path):
    bus = FailedBus(tmp_path)
    bus.data.mkdir()
    (bus.data / "state.json").write_text("failure state")
    callback = bus.data / "callbacks/launch/event.json"
    callback.parent.mkdir(parents=True)
    callback.write_text("callback witness")
    (tmp_path / "tui.log").write_text("failure viewport")
    outside = tmp_path / "outside"
    outside.write_text("must not follow")
    (bus.data / "linked-secret").symlink_to(outside)
    evidence = {"error": "bootstrap failed", "bootstrap": {"message_id": 3}}
    restart.capture_restart_failure(bus, tmp_path, evidence)
    restart.complete_restart_fixture(bus, tmp_path, evidence, RuntimeError("bootstrap failed"))
    assert bus.stopped
    snapshot = tmp_path / "failure-before-cleanup"
    assert (snapshot / "data/state.json").read_text() == "failure state"
    assert (snapshot / "data/callbacks/launch/event.json").read_text() == "callback witness"
    assert (snapshot / "tui.log").read_text() == "failure viewport"
    assert not (snapshot / "data/linked-secret").exists()
    saved = json.loads((tmp_path / "evidence.json").read_text())
    assert saved["failure_capture"]["bootstrap_status"]["message_id"] == "3"
    assert saved["failure_capture"]["native"]["launch_pending"]
    assert saved["failure_capture"]["observe"]["text"] == "startup prompt"
    assert saved["failure_capture"]["phase"] == "before_cleanup"
    assert saved["failure_capture"]["capture_ms"] >= 0


def test_failed_diagnostic_reads_and_writes_do_not_replace_original_error(tmp_path):
    bus = FailedBus(tmp_path, fail_reads=True)
    bus.data.mkdir()
    (tmp_path / "evidence.json").mkdir()  # Force a diagnostic write error.
    original = RuntimeError("original startup timeout")
    evidence = {"error": str(original), "bootstrap": {"message_id": 3}}
    with pytest.raises(RuntimeError) as caught:
        try:
            raise original
        except RuntimeError:
            restart.capture_restart_failure(bus, tmp_path, evidence)
            restart.complete_restart_fixture(bus, tmp_path, evidence, original)
            raise
    assert caught.value is original
    assert bus.stopped
    assert evidence["failure_capture"]["errors"]["state"] == "control unavailable"


@pytest.mark.parametrize("sequenced", [False, True], ids=["legacy", "shared-sequence"])
def test_opt_in_retains_only_failure_and_cleans_success(sequenced):
    name = (f"rf{os.getpid():x}-{time.time_ns() // 1000:x}-1" if sequenced
            else f"brf-{uuid.uuid4().hex[:16]}")
    root = Path("/private/tmp") / name
    with restart.restart_directory(root, True) as run:
        assert run == root
        assert os.stat(run).st_mode & 0o777 == 0o700
        (run / "success").write_text("done")
    assert not root.exists()
    try:
        with pytest.raises(RuntimeError, match="original failure"):
            with restart.restart_directory(root, True) as run:
                (run / "evidence.json").write_text("failure")
                raise RuntimeError("original failure")
        assert (root / "evidence.json").read_text() == "failure"
    finally:
        shutil.rmtree(root)


def test_cleanup_error_after_failed_diagnostics_keeps_original_exception(tmp_path):
    class CleanupErrorBus(FailedBus):
        def stop(self):
            super().stop()
            raise OSError("cleanup transport unavailable")

    bus = CleanupErrorBus(tmp_path, fail_reads=True)
    bus.data.mkdir()
    original = RuntimeError("original bootstrap error")
    evidence = {"error": str(original)}
    with pytest.raises(RuntimeError) as caught:
        try:
            raise original
        except RuntimeError:
            restart.capture_restart_failure(bus, tmp_path, evidence)
            restart.complete_restart_fixture(bus, tmp_path, evidence, original)
            raise
    assert caught.value is original
    assert bus.stopped
    assert evidence["failure_capture"]["bootstrap_error"] == str(original)


def test_default_failure_still_removes_ordinary_temporary_directory():
    with pytest.raises(RuntimeError):
        with restart.restart_directory(None, False) as run:
            root = run
            raise RuntimeError("failure without opt in")
    assert not root.exists()


def test_encoded_socket_limit_and_owned_root_checks_precede_writes():
    # Character count fits; encoded native socket path exceeds 96 bytes.
    long_root = Path("/private/tmp") / ("brf-" + "中" * 20)
    with pytest.raises(ValueError, match="96"):
        with restart.restart_directory(long_root, True, reply=True):
            pytest.fail("must reject before creating or running anything")
    assert not long_root.exists()
    with tempfile.TemporaryDirectory(prefix="rf-unit-", dir="/private/tmp") as parent:
        foreign = Path(parent) / "brf-foreign"
        with pytest.raises(ValueError, match="/private/tmp"):
            with restart.restart_directory(foreign, True):
                pytest.fail("must reject foreign root")
        assert not foreign.exists()
    with tempfile.TemporaryDirectory(prefix="brf-", dir="/private/tmp") as existing:
        marker = Path(existing) / "keep"
        marker.write_text("owned by someone else")
        with pytest.raises(FileExistsError):
            with restart.restart_directory(Path(existing), True):
                pytest.fail("must not adopt an existing root")
        assert marker.read_text() == "owned by someone else"
    with tempfile.TemporaryDirectory(prefix="rf-unit-", dir="/private/tmp") as parent:
        existing = Path(parent).with_name(f"rf{os.getpid():x}-{time.time_ns() // 1000:x}-1")
        existing.symlink_to(parent, target_is_directory=True)
        try:
            with pytest.raises(FileExistsError):
                with restart.restart_directory(existing, True):
                    pytest.fail("must not adopt an existing sequenced root or symlink")
            assert existing.is_symlink()
        finally:
            existing.unlink()


def test_client_socket_suffix_is_checked_even_when_api_socket_fits():
    root = Path("/private/tmp") / ("brf-" + "a" * 25)
    case = root / "claude/data/herdr-config/sessions/bus"
    assert len(os.fsencode(case / "herdr.sock")) <= 96
    assert len(os.fsencode(case / "herdr-client.sock")) > 96
    with pytest.raises(ValueError, match="herdr-client.sock"):
        with restart.restart_directory(root, True, reply=True):
            pytest.fail("the longer client socket must reject before claiming root")
    assert not root.exists()


def test_actual_rust_wrapper_roots_pass_the_python_entry_boundary():
    # Exercise the shared producer's literals and full sequence at one clock tick.
    repo = Path(__file__).resolve().parents[2]
    source = (repo / "tests/tui/native_restart_test.rs").read_text()
    factory = (repo / "src/utils/tests/temp_path_test.rs").read_text()
    label = re.search(r'let fixture = .*unique_temp_path\("([^\"]+)"\)', source).group(1)
    template = re.search(r'"(b\{:x\}-\{:x\}-\{:x\}-\{label\})"', factory).group(1)
    compact = re.search(r'with_file_name\(format!\("([^\"]+)"\)\)', source).group(1)
    assert f'strip_suffix("-{label}")' in source and "strip_prefix('b')" in source
    suffix = re.search(r'let socket = root\.join\("([^\"]+)"\)', source).group(1)
    for pid in (0x1234, 0x12345, 0xFFFFFF):
        for micros in (time.time_ns() // 1000, 4_102_444_800_000_000):
            roots = []
            for sequence in (1, 4096):
                name = template.format(pid, micros, sequence, label=label)
                identity = name.removeprefix("b").removesuffix(f"-{label}")
                root = Path("/private/tmp") / compact.format(identity=identity)
                roots.append(root)
                assert len(os.fsencode(root / suffix)) <= 96
                with restart.restart_directory(root, True, reply=True):
                    assert root.is_dir()
                assert not root.exists()
            assert roots[0] != roots[1], "same-clock sequence identity must survive compaction"
