import importlib.util
from pathlib import Path
import pytest

SPEC = importlib.util.spec_from_file_location(
    "proposed_provenance", Path(__file__).resolve().parents[1] / "root_provenance.py"
)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

TRIAGE = "**模式**：pr\n\n## APPLY\n\n- C1 — accepted root\n\n## REJECT\n\n- C2 — rejected root\n"


def test_canonical_claim_projection_is_deterministic_and_preserves_opaque_test_ids():
    claims = [
        {"claim_id": "C2", "outcome": "REJECT", "sources": ["reviewer@1:2"]},
        {
            "claim_id": "C1",
            "outcome": "APPLY",
            "sources": [
                "reviewer@1:1",
                "run@failed.json#tests/x.py::test[one,two|three]",
            ],
        },
    ]
    table = MODULE.render_provenance(claims, TRIAGE)
    assert table == MODULE.render_provenance(list(reversed(claims)), TRIAGE)
    parsed = MODULE.parse_provenance(TRIAGE + "\n" + table)
    assert [x["claim_id"] for x in parsed] == ["C1", "C2"]
    assert parsed[0]["sources"] == sorted(claims[1]["sources"])


def test_projection_rejects_new_judgment_fields_and_inconsistent_decisions():
    for claims in [
        [{"claim_id": "C1", "outcome": "APPLY", "sources": [], "rating": 10}],
        [{"claim_id": "C1", "outcome": "REJECT", "sources": []}],
        [{"claim_id": "ghost", "outcome": "APPLY", "sources": []}],
        [
            {
                "claim_id": "C1",
                "outcome": "APPLY",
                "sources": ["reviewer@1:1", "reviewer@1:1"],
            }
        ],
    ]:
        with pytest.raises(ValueError):
            MODULE.render_provenance(claims, TRIAGE)


def test_owned_reader_accepts_published_legacy_source_cells_and_ignores_fenced_tables():
    table = "## Root cause provenance\n\n| claim_id | outcome | sources |\n|---|---|---|\n| C1 | APPLY | first@1:1, second@1:2 |\n"
    assert MODULE.parse_provenance(TRIAGE + "\n" + table)[0]["sources"] == [
        "first@1:1",
        "second@1:2",
    ]
    assert MODULE.parse_provenance(TRIAGE + "\n```markdown\n" + table + "```\n") is None


def test_owned_schema_rejects_wrong_columns_and_claim_prefix_matches():
    wrong = "| claim_id | verdict | refs |\n|---|---|---|\n| C1 | APPLY | [] |\n"
    with pytest.raises(ValueError):
        MODULE.parse_provenance(TRIAGE + "\n## Root cause provenance\n\n" + wrong)
    with pytest.raises(ValueError):
        MODULE.render_provenance(
            [dict(claim_id="C1", outcome="APPLY", sources=[])],
            TRIAGE.replace("- C1 —", "- C1-other —"),
        )


def test_scope_accessor_preserves_the_existing_machine_namespace():
    assert MODULE.provenance_scope(TRIAGE) is None
    assert (
        MODULE.provenance_scope("**范围哈希**：" + "a" * 64 + "\n" + TRIAGE) == "a" * 64
    )
    with pytest.raises(ValueError):
        MODULE.provenance_scope("**范围哈希**：invalid\n" + TRIAGE)
