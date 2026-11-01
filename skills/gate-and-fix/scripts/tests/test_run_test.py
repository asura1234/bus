"""The shared entrypoint preserves full-gate scope and parallel build ownership."""
import json
import os
import select as io_select
import signal
import subprocess
import sys
import time
from pathlib import Path
from unittest.mock import patch

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import suite_run as runner  # noqa: E402
from suite_selection import Selection, select  # noqa: E402


def test_full_build_phases_keep_normal_and_instrumented_builds_independent():
    build, tests = runner.commands(select([], full=True))
    assert {gate.name for gate in build} == {"lint", "instrumented-build"}
    assert {gate.name for gate in tests} == {"unit", "integration", "python", "dev-tools", "cli-smoke"}
    assert "none()" in next(g.argv for g in build if g.name == "instrumented-build")
    unit = next(g for g in tests if g.name == "unit")
    integration = next(g for g in tests if g.name == "integration")
    assert "--bin" in unit.argv and "bus" in unit.argv
    assert "not test(/^server::tests::/)" in unit.argv
    assert "kind(test) | test(/^server::tests::/)" in integration.argv
    assert unit.env["LLVM_PROFILE_FILE_NAME"] != integration.env["LLVM_PROFILE_FILE_NAME"]
    assert all("--features" not in g.argv for g in (unit, integration))
    assert "--features" in next(g.argv for g in tests if g.name == "dev-tools")


def test_python_only_selection_does_not_start_cargo():
    build, tests = runner.commands(Selection(False, "none()", ("skills/best-of-n",), False, "pure tooling"))
    assert not build
    assert len(tests) == 1 and tests[0].name == "python"
    assert all(path.startswith("skills/best-of-n/") for path in tests[0].argv[6:])


def test_empty_python_owner_does_not_launch_pytest(tmp_path):
    (tmp_path / "skills/empty").mkdir(parents=True)
    plan = Selection(False, "none()", ("skills/empty",), False, "empty owner")
    with patch.object(runner.quality, "ROOT", tmp_path):
        build, tests = runner.commands(plan)
    assert not build and not tests


def test_empty_python_file_collection_is_success_but_real_failures_stay_red(tmp_path):
    path = tmp_path / "empty_test.py"
    path.write_text("")
    result = runner.batch([runner.Gate("python", (sys.executable, "-m", "pytest", "-q", str(path)))])
    assert result[0].passed
    path.write_text("def test_fails():\n    assert False, 'real regression'\n")
    result = runner.batch([runner.Gate("python", (sys.executable, "-m", "pytest", "-q", str(path)))])
    assert not result[0].passed


def test_independent_tool_test_builds_and_passes_external_cli(tmp_path):
    tools = tmp_path / "tools"
    tools.mkdir()
    (tools / "cli_test.py").write_text('import os\nBINARY = os.environ["BUS_TEST_BINARY"]\n')
    plan = select(["tools/cli_test.py"], root=tmp_path)
    assert not plan.full and plan.binary and plan.rust_filter == "none()"
    target = tmp_path / "external-output"
    with patch.object(runner.quality, "ROOT", tmp_path), patch.dict(os.environ, {"CARGO_TARGET_DIR": str(target)}):
        build, tests = runner.commands(plan)
    assert build[0].argv == ("cargo", "build", "--locked", "--bin", "bus")
    binary = tests[0].env.get("BUS_TEST_BINARY")
    assert binary == str(target / "debug" / ("bus.exe" if os.name == "nt" else "bus"))
    assert not (tmp_path / "target").exists()


def test_affected_cli_keeps_default_binary_separate_from_dev_tools_build():
    plan = Selection(False, "binary(cli)", (), True, "CLI integration")
    assert not plan.coverage_floor
    build, tests = runner.commands(plan)
    assert "llvm-cov" in build[0].argv
    assert "llvm-cov" in next(g.argv for g in tests if g.name == "affected-rust")
    assert "llvm-cov" not in next(g.argv for g in tests if g.name == "dev-tools")


def test_explicit_shared_normal_and_coverage_target_serializes_binary_consumers(tmp_path):
    with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(tmp_path), "CARGO_LLVM_COV_TARGET_DIR": str(tmp_path)}):
        build, tests = runner.commands(select([], full=True))
    assert build[0].resources.intersection(build[1].resources)
    dev = next(gate for gate in tests if gate.name == "dev-tools")
    for gate in tests:
        assert gate.resources.intersection(dev.resources)


def test_partial_run_never_enforces_or_publishes_full_coverage(tmp_path):
    with patch.object(runner, "batch", return_value=[]), patch.object(runner.quality, "coverage") as coverage:
        assert runner.execute(select(["tools/tests/run_launcher_test.py"])) == 0
    coverage.assert_not_called()


def test_failed_test_cannot_reuse_successful_coverage():
    failure = runner.GateResult("unit", ("fake",), 1, 1, "", "failed")
    with patch.object(runner, "batch", side_effect=[[], [failure]]), patch.object(runner.quality, "coverage") as coverage:
        assert runner.execute(select([], full=True)) == 1
    coverage.assert_not_called()


def test_launcher_full_plan_is_auditable_without_any_cargo_process():
    root = Path(__file__).resolve().parents[4]
    result = subprocess.run([str(root / "run"), "test", "--full", "--plan"],
                            cwd=root.parent, text=True, capture_output=True)
    assert result.returncode == 0, result.stderr
    plan = json.loads(result.stdout)
    assert plan["full"] is True
    assert plan["coverage_floor"] is True
    assert plan["head"] == subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()


def test_lane_environment_drops_live_session_but_keeps_explicit_test_binary():
    with patch.dict(os.environ, {"BUS_DATA_DIR": "/live", "HERDR_SESSION_ID": "live"}):
        _, tests = runner.commands(select([], full=True))
    for gate in tests:
        assert "BUS_DATA_DIR" not in gate.env
        assert "HERDR_SESSION_ID" not in gate.env
    python = next(gate for gate in tests if gate.name == "python")
    assert python.env["BUS_TEST_BINARY"].endswith("/debug/bus.exe" if os.name == "nt" else "/debug/bus")


def _pid_readiness_script(ready):
    pending = ready.with_suffix(".tmp")
    return (
        f"Path({str(pending)!r}).write_text(str(os.getpid()));"
        f"Path({str(pending)!r}).replace({str(ready)!r})"
    )


@pytest.mark.skipif(os.name != "posix", reason="POSIX pipe readiness")
def test_child_pid_is_published_only_after_its_write_finishes(tmp_path):
    ready = tmp_path / "ready"
    script = (
        "import os,sys;from pathlib import Path\n"
        "real_open=Path.open\n"
        "def paused_open(path,*args,**kwargs):\n"
        "    handle=real_open(path,*args,**kwargs)\n"
        "    print('opened',flush=True)\n"
        "    assert sys.stdin.buffer.read(1)==b'x'\n"
        "    return handle\n"
        "Path.open=paused_open\n"
        f"{_pid_readiness_script(ready)}\n"
    )
    process = subprocess.Popen([sys.executable, "-c", script], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        # Hold the real open-before-write window until visibility is checked.
        assert io_select.select([process.stdout], [], [], 5)[0], "writer did not open its file"
        assert process.stdout.readline() == b"opened\n"
        assert not ready.exists(), "readiness became visible before its PID was written"
        _, stderr = process.communicate(input=b"x", timeout=5)
        assert process.returncode == 0, stderr
        assert ready.read_text() == str(process.pid)
    finally:
        if process.poll() is None:
            process.kill()
        process.communicate(timeout=5)


@pytest.mark.skipif(os.name != "posix", reason="POSIX process-group teardown")
def test_interrupt_parallel_entrypoint_stops_child_before_deleting_profiles(tmp_path):
    scripts = Path(__file__).resolve().parents[1]
    ready = tmp_path / "ready"
    child = (
        "import os,signal,time;from pathlib import Path;"
        "signal.signal(signal.SIGTERM,signal.SIG_IGN);"
        "(Path(os.environ['BUS_COVERAGE_PROFILE_DIR'])/'unit.profraw').write_bytes(b'counters');"
        f"{_pid_readiness_script(ready)};time.sleep(60)"
    )
    script = (
        f"import sys;sys.path.insert(0,{str(scripts)!r});import suite_run as r;"
        "from pathlib import Path;import os;"
        f"r.quality.ROOT=Path({str(tmp_path)!r});r.quality.OUTPUT=r.quality.ROOT/'reports';"
        f"r.commands=lambda _s:([r.Gate('build',(sys.executable,'-c',{child!r}),env=dict(os.environ))],[]);"
        "sys.exit(r.main(['--full']))"
    )
    env = {key: value for key, value in os.environ.items() if not key.startswith(("BUS_", "HERDR_"))}
    env.update(CARGO_TARGET_DIR=str(tmp_path / "target"), CARGO_LLVM_COV_TARGET_DIR=str(tmp_path / "cov"))
    process = subprocess.Popen([sys.executable, "-c", script], env=env,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    child_pid = None
    try:
        deadline = time.monotonic() + 10
        while not ready.exists() and process.poll() is None and time.monotonic() < deadline:
            time.sleep(.02)
        assert ready.exists(), process.communicate(timeout=1)
        child_pid = int(ready.read_text())
        process.send_signal(signal.SIGTERM)
        _, stderr = process.communicate(timeout=10)
        assert process.returncode == 128 + signal.SIGTERM, stderr
        assert not list((tmp_path / "cov").rglob("*.profraw"))
        assert subprocess.run(["ps", "-p", str(child_pid)], capture_output=True).returncode != 0
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        if child_pid:
            try:
                os.kill(child_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
