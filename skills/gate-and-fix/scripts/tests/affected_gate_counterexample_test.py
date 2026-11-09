"""Counterexamples for --check skipping clippy and the schema golden test."""
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import suite_run as runner  # noqa: E402
import suite_selection as selection  # noqa: E402

ROOT = Path(__file__).resolve().parents[4]


@pytest.fixture(scope="module", autouse=True)
def shared_repository_index():
    index = selection.References(ROOT)
    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(selection, "References", lambda _root: index)
        yield


@pytest.mark.parametrize("path", [
    "tests/tui/driver_test.rs",
    "src/devtools/tui/tests/args_test.rs",
    "src/devtools/tests/disk_space_test.rs",
])
def test_check_on_devtools_only_rust_keeps_clippy_and_schema_once(path):
    plan = selection.select([path], root=ROOT)
    build, tests = runner.commands(plan, checks=True)
    lint = next(gate for gate in build if gate.name == "lint")
    schema_gates = [
        gate.name for gate in (*build, *tests)
        if any("generated_protocol_schema_artifact_is_current" in arg for arg in gate.argv)
    ]
    observed = (plan.full, plan.rust_filter, plan.dev_tools, "--skip-clippy" in lint.argv, len(schema_gates))
    assert observed == (False, "none()", True, False, 1), (observed, schema_gates)
