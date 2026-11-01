"""Selection must never silently narrow unknown or shared inputs."""
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import suite_selection as selection  # noqa: E402


@pytest.mark.parametrize("path", [
    "src/main.rs", "src/protocol/types.rs", "src/client/rooms/ui.rs",
    "Cargo.toml", "Cargo.lock", "build.rs", "build_identity.rs", "run",
    ".cargo/config.toml", "rust-toolchain.toml", "vendor/new.rs",
    "skills/gate-and-fix/scripts/bus_quality.py", "unrecognized.file",
])
def test_shared_or_unknown_changes_require_every_suite(path):
    plan = selection.select([path])
    assert plan.full
    assert plan.rust_filter is None
    assert plan.python == ("scripts", "skills", "cli_extensions", "tools", "packaging")
    assert plan.dev_tools


def test_test_only_module_change_selects_owning_rust_namespace():
    plan = selection.select(["src/agents/providers/codex/tests/hooks_test.rs"])
    assert not plan.full
    assert plan.rust_filter == "(binary(bus) & test(/^agents::/))"
    assert plan.python == ()


def test_integration_fixture_selects_all_targets_that_share_its_directory():
    plan = selection.select(["tests/client/room_screen_test.rs"])
    assert plan.rust_filter == "binary(client) | binary(room-screen)"
    assert not plan.full


def test_shared_rust_fixture_falls_back_to_all_suites():
    assert selection.select(["tests/common/mod.rs"]).full


def test_dev_tool_test_changes_do_not_skip_the_feature_variant():
    plan = selection.select(["src/devtools/tests/screens_test.rs", "tests/tui/driver_test.rs"])
    assert plan.dev_tools
    assert not plan.full


def test_python_owner_runs_its_suite_without_building_rust():
    plan = selection.select(["skills/best-of-n/scripts/ledger.py"])
    assert plan.python == ("skills/best-of-n",)
    assert plan.rust_filter == "none()"
    assert not plan.dev_tools


def test_full_flag_cannot_be_narrowed_by_document_only_changes():
    plan = selection.select(["README.md"], full=True)
    assert plan.full and plan.rust_filter is None and plan.dev_tools
    assert plan.coverage_floor


def test_affected_full_fallback_does_not_enable_a_coverage_verdict():
    assert not selection.select(["src/main.rs"]).coverage_floor


@pytest.mark.parametrize("path", ["/outside", "../outside", "src/../file", "./README.md"])
def test_invalid_paths_are_rejected(path):
    with pytest.raises(ValueError, match="relative"):
        selection.select([path])


def test_changed_paths_include_both_rename_sides_and_every_worktree_state(tmp_path):
    def git(*args):
        return subprocess.check_output(["git", "-C", str(tmp_path), *args], text=True).strip()
    git("init", "-q", "-b", "master")
    git("config", "user.name", "Test")
    git("config", "user.email", "test@example.test")
    for name in ("old.py", "staged.py", "unstaged.py", "deleted.py"):
        (tmp_path / name).write_text("before\n")
    git("add", "old.py", "staged.py", "unstaged.py", "deleted.py")
    git("commit", "-qm", "base")
    base = git("rev-parse", "HEAD")
    git("mv", "old.py", "renamed with space.py")
    (tmp_path / "staged.py").write_text("after\n")
    git("add", "staged.py")
    (tmp_path / "unstaged.py").write_text("after\n")
    (tmp_path / "deleted.py").unlink()
    (tmp_path / "new.py").write_text("new\n")
    assert set(selection.changed_paths(tmp_path, base)) == {
        "old.py", "renamed with space.py", "staged.py", "unstaged.py", "deleted.py", "new.py",
    }
