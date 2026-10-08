"""Option sources are the only basis for the self-selection gate and later tracing; when missing they must be blocked at validation."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest


SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

from best_of_n import main, render, validate  # noqa: E402


def ledger():
    return {
        "goal": "Clicking the preview can select the PiP layer",
        "sources": ["claude", "codex", "cursor"],
        "adjudicator": "claude",
        "disputes": [
            {
                "id": "C3",
                "verdict": "clear",
                "question": "Who releases retainVisiblePreview when pause is accepted",
                "options": [
                    {
                        "id": "A",
                        "tldr": "the transport owner releases it uniformly",
                        "sources": ["claude", "cursor"],
                        "cost": "changes the meaning of settleAtPlayhead:false",
                        "verdict": "chosen",
                        "rank": 1,
                    },
                    {
                        "id": "B",
                        "tldr": "the gesture owner clears it on cancel",
                        "sources": ["codex"],
                        "cost": "adds a new path that writes causal state",
                        "verdict": "ranked",
                        "rank": 2,
                    },
                ],
                "beats_runner_up": "B needs a new cross-module write path; A only lifts an existing check",
                "self_selection_note": "the simpler rival B opens a new write path bypassing the existing owner",
                "simpler_absent_check": "Checked, no simpler form",
                "failure_trigger": "after a scrub cancel, clicking the preview still cannot select PiP",
            }
        ],
    }


def test_winner_without_sources_still_requires_the_self_selection_note():
    """Whether the self-selection gate still holds when the adjudicator omits the winning option's sources."""
    data = ledger()
    del data["disputes"][0]["options"][0]["sources"]
    del data["disputes"][0]["self_selection_note"]

    errors = validate(data)

    assert any("self_selection_note" in e for e in errors), "the winning option has no sources and the self-selection gate was skipped entirely"


def test_winner_with_empty_sources_still_requires_the_self_selection_note():
    data = ledger()
    data["disputes"][0]["options"][0]["sources"] = []
    del data["disputes"][0]["self_selection_note"]

    errors = validate(data)

    assert any("self_selection_note" in e for e in errors), "the winning option's sources is an empty array and the self-selection gate was skipped entirely"


def test_a_sourceless_option_never_reaches_render():
    """An option with missing sources must be blocked at validation, not rendered as "—".

    The original probe's path was "validate lets it through → render writes a '—'". The fix is not to
    make render invent a name, but to make such a record fail validation altogether: sources are the
    only basis for the self-selection gate and later tracing, and without them this adjudication cannot
    be reconstructed. So this asserts the blocking point, and also pins that "a record that passes
    validation never contains empty sources".
    """
    data = ledger()
    del data["disputes"][0]["options"][0]["sources"]
    del data["disputes"][0]["self_selection_note"]

    errors = validate(data)
    assert any("missing sources" in e for e in errors), errors

    # Converse: with sources restored, validation passes and the render has no empty sources.
    data["disputes"][0]["options"][0]["sources"] = ["claude", "cursor"]
    data["disputes"][0]["self_selection_note"] = "the simpler rival loses on mechanism X"
    assert validate(data) == []
    assert "| —" not in render(data)


def test_next_subcommand_reports_a_malformed_ledger_instead_of_crashing(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
):
    """Subcommands other than record did not run validate; which failure path does a record missing rank take."""
    data = ledger()
    del data["disputes"][0]["options"][1]["rank"]
    path = tmp_path / "l.json"
    path.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")

    code = main(["next", "--ledger", str(path), "--dispute", "C3", "--failed", "A"])

    # The point is "report, don't crash". Exit code 1 rather than 2, consistent with record's convention:
    # 1 = the record itself is invalid (FAIL plus every reason on stdout), 2 = the record cannot be read.
    assert code == 1, "a malformed ledger should be reported as a validation failure, not a traceback"
    out = capsys.readouterr().out
    assert "FAIL" in out and "rank" in out, out
