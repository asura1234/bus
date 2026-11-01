"""Affected selection narrows proven owners in the actual Bus repository."""
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import suite_selection as selection  # noqa: E402

ROOT = Path(__file__).resolve().parents[4]


@pytest.mark.parametrize("path, expected", [
    # The R1-head selector test example, now against the real tree instead of a tmp fixture.
    ("src/agents/providers/codex/tests/hooks_test.rs", "(binary(bus) & test(/^agents::/))"),
    ("src/server/tests/lifecycle_test.rs", "(binary(bus) & test(/^server::/))"),
    # Only tests/api/api_test.rs includes it.
    ("tests/api/agents_test.rs", "binary(api)"),
])
def test_owned_test_file_narrows_on_real_repo(path, expected):
    plan = selection.select([path], root=ROOT)
    assert not plan.full and plan.rust_filter == expected, plan


def test_tools_test_narrows_on_real_repo():
    plan = selection.select(["tools/tests/sitrep_test.py"], root=ROOT)
    assert not plan.full and plan.python == ("tools",), plan


@pytest.mark.parametrize("path", [
    "tools/acceptance/reply_restart_test.py",
    "tools/acceptance/native_restart_test.py",
    "tools/acceptance/harness_test.py",
])
def test_python_scenario_run_by_tui_driver_selects_dev_tools(path):
    plan = selection.select([path], root=ROOT)
    assert plan.full or plan.dev_tools, plan


def test_src_test_included_through_shared_container_selects_its_integration_binaries():
    plan = selection.select(["src/utils/tests/launch_paths_test.rs"], root=ROOT)
    assert plan.full or all(f"binary({b})" in (plan.rust_filter or "")
                            for b in ("api", "client", "server")), plan
