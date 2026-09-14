import sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from consumer_fallout import _module_specifiers  # noqa: E402
from consumer_fallout_analysis import (  # noqa: E402
    DIRECT_REASON,
    candidate_reasons_by_source,
    is_test,
    source_module_paths,
    test_subject_stem as subject_stem,
)
from review_round_support import prereq_failures  # noqa: E402


def test_rust_module_and_test_paths_are_in_consumer_fallout() -> None:
    source = "src/bus/model.rs"
    importer = "src/client/shell.rs"
    specifiers = _module_specifiers(
        importer,
        """use crate::bus::{
    model::{self, BusState},
    runtime::Runtime,
};
""",
    )

    candidates = candidate_reasons_by_source(
        (source,),
        {importer: specifiers},
        {"model": ("src/bus/model_tests.rs",)},
    )

    assert candidates[source][importer] == {DIRECT_REASON}
    assert "crate::bus::model" in specifiers
    assert is_test("src/bus/model_tests.rs")
    assert subject_stem("src/bus/model_tests.rs") == "model"
    assert "src/bus" in source_module_paths("src/bus/mod.rs")


def test_legacy_plan_gets_one_actionable_migration_failure() -> None:
    failures = prereq_failures(
        "# Legacy plan\n\n- Status: ready-for-review\n",
    )

    assert len(failures) == 1
    assert "legacy plan" in failures[0]
    assert "create-plan" in failures[0]
