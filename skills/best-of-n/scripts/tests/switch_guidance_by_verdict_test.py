"""The switch criterion must vary with the verdict — the next step for clear and for toss-up is not the same thing.

guide.md's "Three conclusions" gives the two opposite instructions: clear must first ask "was the
mechanism the first place won on overturned", while toss-up is "the second place is genuinely alive;
once the first place goes badly, switch early, do not force it".
"""

from __future__ import annotations

import sys
from pathlib import Path


SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

from best_of_n import seeds  # noqa: E402


def ledger(verdict: str) -> dict:
    dispute = {
        "id": "D1",
        "verdict": verdict,
        "question": "q",
        "options": [
            {"id": "A", "tldr": "t1", "sources": ["claude"], "cost": "c", "verdict": "chosen", "rank": 1},
            {"id": "B", "tldr": "t2", "sources": ["codex"], "cost": "c", "verdict": "ranked", "rank": 2},
        ],
        "simpler_absent_check": "Checked",
        "self_selection_note": "B bypasses the existing owner",
        "failure_trigger": "still reproduces",
    }
    if verdict == "clear":
        dispute["beats_runner_up"] = "B adds a cross-module write path"
    else:
        dispute["toss_up_reason"] = "one real trace is missing"
    return {"goal": "g", "sources": ["claude", "codex"], "adjudicator": "claude", "disputes": [dispute]}


def _decision_block(out: str) -> str:
    return out.split("First judge", 1)[1] if "First judge" in out else out


def test_toss_up_and_clear_do_not_share_one_switch_rule() -> None:
    clear = _decision_block(seeds(ledger("clear"), "D1", "A"))
    toss_up = _decision_block(seeds(ledger("toss-up"), "D1", "A"))

    assert clear != toss_up, (
        "toss-up and clear got word-for-word identical switch criteria; guide.md's instruction for toss-up is "
        "\"switch early, do not force it\", while the only non-overturned branch here is \"do not slide down the ranking\""
    )
