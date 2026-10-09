"""When options is written as a list of strings it must fail closed, not crash on AttributeError."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest


SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

from best_of_n import main, validate  # noqa: E402


def test_string_option_is_reported_instead_of_raising() -> None:
    ledger = {
        "goal": "g",
        "sources": ["claude", "codex"],
        "adjudicator": "claude",
        "disputes": [
            {
                "id": "D1",
                "verdict": "clear",
                "question": "q",
                "options": ["A"],
                "beats_runner_up": "x",
                "self_selection_note": "x",
                "simpler_absent_check": "Checked",
                "failure_trigger": "still reproduces",
            }
        ],
    }

    errors = validate(ledger)

    assert errors, "options: ['A'] was accepted as a valid ledger"
    assert all(isinstance(item, str) for item in errors)


def test_next_on_string_options_exits_without_traceback(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    ledger = {
        "goal": "g",
        "sources": ["claude", "codex"],
        "adjudicator": "claude",
        "disputes": [
            {
                "id": "D1",
                "verdict": "clear",
                "question": "q",
                "options": ["A"],
                "beats_runner_up": "x",
                "simpler_absent_check": "Checked",
                "failure_trigger": "still reproduces",
            }
        ],
    }
    path = tmp_path / "ledger.json"
    path.write_text(json.dumps(ledger), encoding="utf-8")

    code = main(["next", "--ledger", str(path), "--dispute", "D1", "--failed", "A"])
    captured = capsys.readouterr()

    assert code in {1, 2}
    assert "Traceback" not in captured.err
    assert "AttributeError" not in captured.err
