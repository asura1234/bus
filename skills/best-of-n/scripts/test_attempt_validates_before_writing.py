"""next already validates first; attempt, as the writer, must not write its result into a record that was never valid."""

from __future__ import annotations

import json
import sys
from pathlib import Path


SCRIPTS_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS_DIR))

from best_of_n import main  # noqa: E402


def test_attempt_refuses_an_invalid_ledger(tmp_path: Path) -> None:
    data = {
        "goal": "g",
        "sources": ["claude", "codex"],
        "adjudicator": "claude",
        "disputes": [
            {
                "id": "D1",
                "verdict": "clear",
                "question": "q",
                "options": [
                    {
                        "id": "A",
                        "tldr": "t1",
                        "sources": ["claude"],
                        "cost": "c",
                        "verdict": "chosen",
                        "rank": 1,
                    },
                    {
                        "id": "B",
                        "tldr": "t2",
                        "sources": ["codex"],
                        "cost": "c",
                        "verdict": "ranked",
                        "rank": 2,
                    },
                ],
                "beats_runner_up": "B adds a cross-module write path",
                "self_selection_note": "B bypasses the existing owner",
            }
        ],
    }
    path = tmp_path / "ledger.json"
    path.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
    before = path.read_text(encoding="utf-8")

    code = main(
        [
            "attempt",
            "--ledger",
            str(path),
            "--dispute",
            "D1",
            "--option",
            "A",
            "--outcome",
            "still reproduces",
        ]
    )

    assert code != 0, "attempt returned success on a ledger missing failure_trigger"
    assert path.read_text(encoding="utf-8") == before
