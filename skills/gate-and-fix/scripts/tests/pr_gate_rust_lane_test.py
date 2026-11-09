"""A dev-tools-only Rust change still has to lint with Clippy and check the schema."""
import sys
from pathlib import Path
from unittest.mock import patch

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import suite_run as runner  # noqa: E402
import suite_selection as selection  # noqa: E402

ROOT = Path(__file__).resolve().parents[4]
SCHEMA = "generated_protocol_schema_artifact_is_current"


@pytest.fixture(scope="module", autouse=True)
def shared_repository_index():
    index = selection.References(ROOT)
    with patch.object(selection, "References", return_value=index):
        yield


@pytest.mark.parametrize("path", [
    "src/devtools/tests/disk_space_test.rs",
    "tests/tui/driver_test.rs",
])
def test_check_keeps_clippy_and_schema_for_devtools_only_rust(path):
    plan = selection.select([path], root=ROOT)
    assert not plan.full and plan.dev_tools and plan.rust_filter == "none()", plan
    build, tests = runner.commands(plan, checks=True)
    lint = next(gate for gate in build if gate.name == "lint")
    schema_gates = [gate.name for gate in (*build, *tests)
                    if any(SCHEMA in arg for arg in gate.argv)]
    assert "--skip-clippy" not in lint.argv and len(schema_gates) == 1, (
        lint.argv, schema_gates, [gate.name for gate in (*build, *tests)])
