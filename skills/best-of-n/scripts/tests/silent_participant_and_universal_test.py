"""How a participant with no approach in the record is handled must not conflict with the universal criterion.

SKILL.md §3 a0 explicitly says: when the disagreement is really about scheduling / PR splitting /
product trade-offs, hand it back to the orchestrator as is, **keep it out of the ledger**. So a
participant who only had an opinion on that dispute necessarily has no option in the persisted record.
"""

from __future__ import annotations

import sys
from pathlib import Path


SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

from best_of_n import validate  # noqa: E402


def ledger(sources: list[str]) -> dict:
    return {
        "goal": "g",
        "sources": sources,
        "adjudicator": "claude",
        "disputes": [
            {
                "id": "D1",
                "verdict": "universal",
                "question": "q",
                "options": [
                    {
                        "id": "A",
                        "tldr": "t1",
                        "sources": ["claude", "codex"],
                        "cost": "c",
                        "verdict": "chosen",
                        "rank": 1,
                    },
                ],
                "simpler_absent_check": "Checked",
                "failure_trigger": "still reproduces",
            }
        ],
    }


def test_participant_whose_only_dispute_went_back_to_the_orchestrator() -> None:
    """A participant handed back to the orchestrator must have a legal notation, and it must not shrink the universal denominator.

    Silence is **ambiguous**: it may mean their approach was silently dropped (the point of the ranking
    is to not drop options), or that the only dispute they had an opinion on was handed back to the
    orchestrator per §3 a0. The validator cannot tell, so it requires an explicit `deferred_sources`
    declaration — stricter than the original probe's "silence is legal", but it protects exactly this
    skill's core invariant.

    The key is that after declaring, **they stay in sources**: that is the universal denominator. So the
    record is no longer rejected for "silence", but it also does **not** become universal because of it —
    cursor never took a position on D1, so "everyone gave the same proposal" is false. It should be clear.
    """

    raw = ledger(["claude", "codex", "cursor"])
    errors = validate(raw)
    assert any("have no corresponding option" in e for e in errors), errors

    declared = ledger(["claude", "codex", "cursor"])
    declared["deferred_sources"] = ["cursor"]
    errors = validate(declared)
    assert not any("have no corresponding option" in e for e in errors), errors
    # The denominator did not change, so universal still does not hold.
    assert any("universal" in e for e in errors), errors

    declared["disputes"][0]["verdict"] = "clear"
    assert validate(declared) == []


def test_no_error_message_ever_suggests_shrinking_sources() -> None:
    """The validator must not present "delete a real participant from sources" as a fix.

    The original probe asserted that "rejected with 3 participants, yet passing once cut to 2" must not
    happen at all. That requires the validator to detect a record that **misreports its participant
    list**, and nothing in the ledger supports that judgment — silence and "never took part" look
    identical in the file. What can be checked is the real harm surface: error messages must not steer
    anyone that way. Shrinking the denominator launders "2 of 3 recommended" into "everyone gave the same
    proposal", which is exactly what the universal criterion exists to prevent.
    """

    errors = validate(ledger(["claude", "codex", "cursor"]))
    silent = [e for e in errors if "have no corresponding option" in e]
    assert silent, errors
    for message in silent:
        assert "deferred_sources" in message
        assert "Do not" in message and "delete them from sources" in message
