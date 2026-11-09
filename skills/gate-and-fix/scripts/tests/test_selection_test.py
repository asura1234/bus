"""Selection must never silently narrow unknown or shared inputs."""
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import suite_selection as selection  # noqa: E402


def put(root, name, text):
    path = root / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


@pytest.mark.parametrize("path", [
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


def test_test_only_module_change_selects_owning_rust_namespace(tmp_path):
    put(tmp_path, "src/main.rs", "mod agents;")
    put(tmp_path, "src/agents/mod.rs", '#[path="tests/local_test.rs"] mod local;')
    put(tmp_path, "src/agents/tests/local_test.rs", "#[test] fn case() {}")
    plan = selection.select(["src/agents/tests/local_test.rs"], root=tmp_path)
    assert not plan.full
    assert plan.rust_filter == "(binary(bus) & test(/^agents::/))"
    assert plan.python == ()


def test_integration_fixture_selects_all_targets_that_reference_it(tmp_path):
    put(tmp_path, "Cargo.toml", '[[test]]\nname="client"\npath="tests/client/client_test.rs"\n[[test]]\nname="room-screen"\npath="tests/client/room_screen_test.rs"')
    put(tmp_path, "tests/client/client_test.rs", '#[path="room_screen_test.rs"] mod shared;')
    put(tmp_path, "tests/client/room_screen_test.rs", "#[test] fn case() {}")
    plan = selection.select(["tests/client/room_screen_test.rs"], root=tmp_path)
    assert plan.rust_filter == "binary(client) | binary(room-screen)"
    assert not plan.full


def test_shared_rust_fixture_falls_back_to_all_suites():
    assert selection.select(["tests/common/mod.rs"]).full


def test_dev_tool_test_changes_do_not_skip_the_feature_variant(tmp_path):
    put(tmp_path, "src/main.rs", "mod devtools;")
    put(tmp_path, "src/devtools/mod.rs", '#[path="tests/screens_test.rs"] mod screens;')
    put(tmp_path, "src/devtools/tests/screens_test.rs", "#[test] fn case() {}")
    put(tmp_path, "Cargo.toml", '[[test]]\nname="tui-driver"\npath="tests/tui/driver_test.rs"')
    put(tmp_path, "tests/tui/driver_test.rs", "#[test] fn case() {}")
    plan = selection.select(["src/devtools/tests/screens_test.rs", "tests/tui/driver_test.rs"], root=tmp_path)
    assert plan.dev_tools
    assert not plan.full


def test_missing_python_skill_fails_closed():
    plan = selection.select(["skills/best-of-n/scripts/ledger.py"])
    assert plan.full and "tools" in plan.python
    assert plan.rust_filter is None and plan.dev_tools


def test_full_flag_keeps_checks_but_skips_languages_for_document_only_changes():
    plan = selection.select(["README.md"], full=True)
    assert plan.full and plan.rust_filter == "none()" and not plan.dev_tools
    assert not plan.python and not plan.coverage_floor


def test_affected_full_fallback_does_not_enable_a_coverage_verdict():
    assert not selection.select(["src/main.rs"]).coverage_floor


def test_production_source_selects_owner_namespace_and_transitive_test_referrers(tmp_path):
    put(tmp_path, "src/main.rs", "mod agents;\nmod client;")
    put(tmp_path, "src/agents/mod.rs", 'mod worker;\n#[path="tests/worker_test.rs"] mod worker_tests;')
    put(tmp_path, "src/agents/worker.rs", "pub fn work() {}")
    put(tmp_path, "src/agents/tests/worker_test.rs", "#[test] fn case() {}")
    put(tmp_path, "src/client/mod.rs", '#[path="tests/links_test.rs"] mod links;')
    put(tmp_path, "src/client/tests/links_test.rs", '#[path="../../main.rs"] mod entry;\n#[test] fn case() {}')
    plan = selection.select(["src/agents/worker.rs"], root=tmp_path)
    assert not plan.full and not plan.coverage_floor
    assert plan.rust_filter == "(binary(bus) & test(/^agents::/)) | (binary(bus) & test(/^client::/))"


def test_integration_include_of_production_source_selects_that_binary(tmp_path):
    put(tmp_path, "Cargo.toml", '[[test]]\nname="api"\npath="tests/api/root_test.rs"\n')
    put(tmp_path, "src/utils/mod.rs", "mod paths;")
    put(tmp_path, "src/utils/paths.rs", "pub fn launch() {}")
    put(tmp_path, "tests/api/root_test.rs", '#[path="../../src/utils/paths.rs"] mod paths;\n#[test] fn case() {}')
    plan = selection.select(["src/utils/paths.rs"], root=tmp_path)
    assert not plan.full
    assert "binary(api)" in plan.rust_filter
    assert "(binary(bus) & test(/^utils::/))" in plan.rust_filter


def test_serde_cli_tui_and_cross_owner_source_stay_full(tmp_path):
    put(tmp_path, "src/main.rs", "mod agents;\nmod messaging;\nmod server;")
    put(tmp_path, "src/agents/mod.rs", "mod row;")
    put(tmp_path, "src/agents/row.rs", "#[derive(Serialize, Deserialize)] struct Row {}")
    put(tmp_path, "src/cli/mod.rs", "mod args;")
    put(tmp_path, "src/cli/args.rs", "pub fn flag() {}")
    put(tmp_path, "src/terminal/mod.rs", "mod view;")
    put(tmp_path, "src/terminal/view.rs", 'pub fn draw() -> &\'static str { "ready" }')
    put(tmp_path, "src/messaging/mod.rs", "mod shape;")
    put(tmp_path, "src/messaging/shape.rs", "pub fn shape() {}")
    put(tmp_path, "src/server/mod.rs", '#[path="../messaging/shape.rs"] mod foreign;')
    assert selection.select(["src/agents/row.rs"], root=tmp_path).full
    assert selection.select(["src/cli/args.rs"], root=tmp_path).full
    assert selection.select(["src/terminal/view.rs"], root=tmp_path).full
    assert selection.select(["src/messaging/shape.rs"], root=tmp_path).full


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
    # Index and working copy can cancel in a combined diff against the base.
    (tmp_path / "staged.py").write_text("before\n")
    (tmp_path / "unstaged.py").write_text("after\n")
    (tmp_path / "deleted.py").unlink()
    (tmp_path / "new.py").write_text("new\n")
    assert set(selection.changed_paths(tmp_path, base)) == {
        "old.py", "renamed with space.py", "staged.py", "unstaged.py", "deleted.py", "new.py",
    }
