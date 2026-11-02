"""Lane eligibility is independent of full versus affected test breadth."""
import json
import sys
from pathlib import Path
from unittest.mock import patch

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import suite_run as runner  # noqa: E402
from suite_selection import select  # noqa: E402


def put(root, name, text=""):
    path = root / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def names(plan, root):
    with patch.object(runner.quality, "ROOT", root):
        build, tests = runner.commands(plan)
    return {gate.name: gate for gate in [*build, *tests]}


@pytest.mark.parametrize("full", [False, True])
def test_python_only_does_not_enable_rust_lanes(tmp_path, full):
    put(tmp_path, "tools/pure_test.py", "def test_case(): pass\n")
    plan = select(["tools/pure_test.py"], full=full, root=tmp_path)
    assert plan.rust_filter == "none()" and not plan.dev_tools
    assert not plan.coverage_floor
    gates = names(plan, tmp_path)
    assert "python" in gates
    assert not any(g.argv[0] == "cargo" for g in gates.values())
    if full:
        assert "--skip-clippy" in gates["lint"].argv


@pytest.mark.parametrize("full", [False, True])
def test_rust_test_only_does_not_enable_python_lane(tmp_path, full):
    put(tmp_path, "src/main.rs", "mod agents;")
    put(tmp_path, "src/agents/mod.rs", '#[path="tests/local_test.rs"] mod local;')
    put(tmp_path, "src/agents/tests/local_test.rs", "#[test] fn case() {}")
    plan = select(["src/agents/tests/local_test.rs"], full=full, root=tmp_path)
    assert not plan.python
    gates = names(plan, tmp_path)
    assert "python" not in gates
    assert ("instrumented-build" if full else "test-build") in gates
    assert plan.coverage_floor == full


@pytest.mark.parametrize("full", [False, True])
def test_docs_skip_both_lanes_but_keep_lint_and_document_contracts(tmp_path, full):
    put(tmp_path, "docs/testing.md", "Documentation\n")
    plan = select(["docs/testing.md"], full=full, root=tmp_path)
    assert not plan.python and plan.rust_filter == "none()"
    assert not plan.coverage_floor and not runner.needs_profiles(plan)
    gates = names(plan, tmp_path)
    assert set(gates) == {"lint", "docs"}
    assert "--skip-clippy" in gates["lint"].argv


@pytest.mark.parametrize("path", [
    "Cargo.toml", "Cargo.lock", "build.rs", "build_identity.rs", "run", "justfile",
    ".github/workflows/check.yml", ".cargo/config.toml", "rust-toolchain.toml",
    "pytest.ini", "conftest.py", "pyproject.toml", "unknown.input",
    "skills/gate-and-fix/scripts/suite_run.py",
])
def test_shared_and_unknown_inputs_enable_both_lanes(tmp_path, path):
    put(tmp_path, path)
    plan = select([path], full=True, root=tmp_path)
    assert plan.python and plan.rust_filter is None and plan.dev_tools
    assert plan.coverage_floor


def test_rust_runtime_change_runs_python_binary_consumers_and_source_readers(tmp_path):
    put(tmp_path, "src/main.rs", "fn main() {}")
    put(tmp_path, "tools/binary_test.py", 'import os\nbinary=os.environ["BUS_TEST_BINARY"]\n')
    put(tmp_path, "tools/source_test.py", 'from pathlib import Path\nSOURCE=Path(__file__).parents[1]/"src/main.rs"\n')
    put(tmp_path, "tools/unrelated_test.py", "def test_unrelated(): pass\n")
    plan = select(["src/main.rs"], full=True, root=tmp_path)
    assert set(plan.python) == {"tools/binary_test.py", "tools/source_test.py"}
    gates = names(plan, tmp_path)
    assert "tools/binary_test.py" in gates["python"].argv
    assert "tools/source_test.py" in gates["python"].argv
    assert "tools/unrelated_test.py" not in gates["python"].argv


def test_python_binary_consumer_builds_cli_without_rust_test_or_coverage_lanes(tmp_path):
    put(tmp_path, "tools/cli_test.py", 'import os\nbinary=os.environ["BUS_TEST_BINARY"]\n')
    plan = select(["tools/cli_test.py"], full=True, root=tmp_path)
    gates = names(plan, tmp_path)
    assert "cli-build" in gates and "python" in gates
    assert plan.rust_filter == "none()" and not plan.dev_tools and not plan.coverage_floor
    assert not runner.needs_profiles(plan)
    assert not {"unit", "integration", "dev-tools", "instrumented-build"}.intersection(gates)


def test_python_helper_used_by_rust_driver_keeps_dependent_rust_tests(tmp_path):
    put(tmp_path, "Cargo.toml", '[[test]]\nname="tui-driver"\npath="tests/driver_test.rs"\n')
    put(tmp_path, "tests/driver_test.rs", 'fn case() { Command::new("python3").arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tools/scenario_test.py")); }')
    put(tmp_path, "tools/scenario_test.py", "from .helper import run\n")
    put(tmp_path, "tools/helper.py", "def run(): pass\n")
    plan = select(["tools/helper.py"], full=True, root=tmp_path)
    assert plan.rust_filter is None and plan.dev_tools and plan.python and plan.coverage_floor


def test_embedded_doc_is_a_rust_input(tmp_path):
    put(tmp_path, "src/main.rs", 'const HELP: &str = include_str!("../docs/help.md");')
    put(tmp_path, "docs/help.md", "Help\n")
    plan = select(["docs/help.md"], full=True, root=tmp_path)
    assert plan.rust_filter is None and plan.coverage_floor


def test_deleted_python_input_fails_closed_for_lost_dependency_edges(tmp_path):
    plan = select(["tools/deleted.py"], full=True, root=tmp_path)
    assert plan.python and plan.rust_filter is None


def test_docs_keep_transitive_document_contract_consumers_outside_python_lane(tmp_path):
    put(tmp_path, "docs/help.md", "Help\n")
    put(tmp_path, "tools/doc_helper.py", 'from pathlib import Path\nDOC=Path(__file__).parents[1]/"docs/help.md"\n')
    put(tmp_path, "tools/doc_test.py", "from .doc_helper import DOC\n")
    plan = select(["docs/help.md"], full=True, root=tmp_path)
    assert not plan.python and plan.rust_filter == "none()"
    assert plan.doc_tests == ("tools/doc_test.py",)
    gates = names(plan, tmp_path)
    assert "tools/doc_test.py" in gates["doc-consumers"].argv
    assert "python" not in gates


@pytest.mark.parametrize("full", [False, True])
@pytest.mark.parametrize("walk", ['glob("*.md")', 'rglob("*.md")', 'iterdir()'])
def test_document_directory_readers_remain_document_checks(tmp_path, full, walk):
    put(tmp_path, "docs/help.md", "Help\n")
    put(tmp_path, "tools/doc_helper.py", 'from pathlib import Path\n'
        f'PAGES=list((Path(__file__).parents[1]/"docs").{walk})\n')
    put(tmp_path, "tools/doc_test.py", "from .doc_helper import PAGES\n")
    put(tmp_path, "tools/unrelated_test.py", "def test_unrelated(): pass\n")
    plan = select(["docs/help.md"], full=full, root=tmp_path)
    assert not plan.python and plan.rust_filter == "none()"
    assert plan.doc_tests == ("tools/doc_test.py",)
    assert "tools/doc_test.py" in names(plan, tmp_path)["doc-consumers"].argv


@pytest.mark.parametrize("full", [False, True])
def test_deleted_document_fails_closed_for_lost_dependency_edges(tmp_path, full):
    put(tmp_path, "tools/doc_test.py", 'from pathlib import Path\n'
        'DOC=Path(__file__).parents[1]/"docs/removed.md"\n'
        'def test_document_exists(): assert DOC.is_file()\n')
    plan = select(["docs/removed.md"], full=full, root=tmp_path)
    assert plan.python and plan.rust_filter is None and plan.dev_tools
    assert plan.coverage_floor == full
    gates = names(plan, tmp_path)
    assert "tools/doc_test.py" in gates["python"].argv


@pytest.mark.parametrize("full", [False, True])
def test_shipped_guide_keeps_the_installed_bundle_link_contract(full):
    root = Path(__file__).resolve().parents[4]
    plan = select(["skills/gate-and-fix/guide.md"], full=full, root=root)
    assert "tools/tests/installed_skills_recovery_test.py" in plan.doc_tests


def test_plan_and_report_describe_skips_and_do_not_evaluate_coverage(tmp_path, capsys):
    put(tmp_path, "docs/testing.md", "Docs\n")
    with (patch.object(runner.quality, "ROOT", tmp_path),
          patch.object(runner.quality, "head", return_value="head"),
          patch.object(runner, "changed_paths", return_value=["docs/testing.md"]) as paths):
        assert runner.main(["--full", "--base", "baseline", "--plan"]) == 0
        paths.assert_called_once_with(tmp_path, "baseline")
        plan = json.loads(capsys.readouterr().out)
        assert plan["lanes"]["python"] == {"status": "SKIPPED", "reason": "no Python changes vs baseline"}
        assert plan["lanes"]["rust"] == {"status": "SKIPPED", "reason": "no Rust changes vs baseline"}
        with (patch.object(runner, "batch", return_value=[]),
              patch.object(runner.quality, "coverage") as coverage):
            assert runner.execute(select(["docs/testing.md"], full=True, root=tmp_path), base="baseline") == 0
        coverage.assert_not_called()
    report = json.loads(next((tmp_path / "temp/run-test").glob("*.json")).read_text())
    assert report["lanes"]["rust"]["status"] == "SKIPPED"
    assert report["coverage"] == {"status": "SKIPPED", "reason": "no Rust changes vs baseline"}
    assert "SKIPPED" in capsys.readouterr().out


def test_full_mode_rejects_an_invalid_base_before_starting_any_work():
    with (patch.object(runner, "changed_paths", side_effect=ValueError("invalid base")),
          patch.object(runner, "execute") as execute):
        assert runner.main(["--full", "--base", "missing"]) == 1
    execute.assert_not_called()


def test_lint_without_rust_keeps_all_static_checks_but_omits_both_clippy_variants():
    with (patch.object(runner.quality, "run", return_value=0) as run,
          patch.object(runner.quality, "file_lengths", return_value=0)):
        assert runner.quality.lint(skip_clippy=True) == 0
    calls = [call.args for call in run.call_args_list]
    assert ("cargo", "fmt", "--check") in calls
    assert not any("clippy" in argv for argv in calls)
    assert any("ruff" in argv for argv in calls)
    assert any("tools.quality.import_boundaries" in argv for argv in calls)


def test_failed_preflight_cannot_report_passing_rust_from_only_dev_tools(tmp_path):
    failure = runner.GateResult("instrumented-build", ("fake",), 1, 1, "", "failed")
    success = runner.GateResult("dev-tools", ("fake",), 0, 1, "", "")
    with (patch.object(runner.quality, "ROOT", tmp_path),
          patch.object(runner.quality, "head", return_value="head"),
          patch.object(runner, "batch", side_effect=[[failure], [success]])):
        assert runner.execute(select(["run"], full=True, root=tmp_path)) == 1
    report = json.loads(next((tmp_path / "temp/run-test").glob("*.json")).read_text())
    assert report["lanes"]["rust"]["status"] == "FAIL"
    assert report["lanes"]["python"]["status"] == "NOT_RUN"


def test_devtools_only_affected_run_does_not_claim_no_rust_changes(tmp_path):
    put(tmp_path, "src/main.rs", "mod devtools;")
    put(tmp_path, "src/devtools/mod.rs", '#[path="tests/local_test.rs"] mod local;')
    put(tmp_path, "src/devtools/tests/local_test.rs", "#[test] fn case() {}")
    plan = select(["src/devtools/tests/local_test.rs"], root=tmp_path)
    assert plan.dev_tools and plan.rust_filter == "none()"
    success = runner.GateResult("dev-tools", ("probe",), 0, 1, "", "")
    with (patch.object(runner.quality, "ROOT", tmp_path),
          patch.object(runner.quality, "head", return_value="head"),
          patch.object(runner, "batch", side_effect=[[], [success]])):
        assert runner.execute(plan) == 0
    report = json.loads(next((tmp_path / "temp/run-test").glob("*.json")).read_text())
    assert report["lanes"]["rust"]["status"] == "PASS"
    assert report["coverage"] == {"status": "SKIPPED", "reason": "affected mode"}
