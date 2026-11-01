"""Affected selection covers shared Rust, embedded data and tooling consumers."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import suite_selection as selection  # noqa: E402


def test_shared_src_test_helper_reaches_integration_binaries():
    # tests/{api,cli,client,server,tui}/*_test.rs and room_screen_test.rs all #[path]-include this file.
    plan = selection.select(["src/utils/tests/live_bus_env_test.rs"])
    assert plan.full or all(f"binary({b})" in plan.rust_filter
                            for b in ("api", "cli", "client", "room-screen", "server")), plan


def test_embedded_markdown_reaches_rust_tests():
    # src/messaging/orchestration.rs include_str!s orchestration/rules.md; messaging tests assert on it.
    plan = selection.select(["orchestration/rules.md"])
    assert plan.full or plan.rust_filter != "none()", plan


def test_dev_tools_doc_reaches_dev_tools_tests():
    # src/devtools/tui/tests/args_test.rs compares docs/tui-driver.md with the shipped help.
    plan = selection.select(["docs/tui-driver.md"])
    assert plan.full or plan.dev_tools, plan


def test_skill_script_reaches_tools_tests_that_exercise_it():
    # tools/tests/review_pr_round_test.py runs skills/review-pr/scripts/review_round.py.
    plan = selection.select(["skills/review-pr/scripts/review_round.py"])
    assert plan.full or "tools" in plan.python, plan


def test_skill_without_own_tests_still_runs_its_tests():
    # skills/sitrep has no *_test.py; its tests are tools/tests/sitrep_test.py.
    plan = selection.select(["skills/sitrep/scripts/render.py"])
    assert plan.full or "tools" in plan.python, plan


def test_python_only_affected_run_still_gets_a_built_cli():
    # tools/tests/bus_dev_test.py:10 falls back to <repo>/target/debug/bus (stale/absent under a shared CARGO_TARGET_DIR).
    import suite_run as runner
    _, tests = runner.commands(selection.select(["tools/tests/bus_dev_test.py"]))
    python = next(g for g in tests if g.name == "python")
    binary = python.env.get("BUS_TEST_BINARY")
    assert binary, python.argv
