"""Switching: options already attempted must not become seeds again, and toss-up must still yield a next step."""

from __future__ import annotations

import sys
from pathlib import Path


SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

from best_of_n import seeds  # noqa: E402


def ledger_with_documented_outcome_log() -> dict:
    """outcome_log is taken verbatim from the ledger schema example in best_of_n.py's module docstring."""

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
                    {
                        "id": "C",
                        "tldr": "the store subscriber clears it as a fallback",
                        "sources": ["claude"],
                        "cost": "introduces a second source of truth",
                        "verdict": "ranked",
                        "rank": 3,
                    },
                ],
                "beats_runner_up": "B needs a new cross-module write path; A only lifts an existing check",
                "self_selection_note": "the simpler rival B opens a new write path bypassing the existing owner",
                "simpler_absent_check": "Checked, no simpler form",
                "failure_trigger": "after a scrub cancel, clicking the preview still cannot select PiP",
                "outcome_log": ["2026-09-16 B failed: the cancel path was never triggered, switching to A"],
            }
        ],
    }


def test_option_already_failed_in_outcome_log_is_not_offered_as_a_seed() -> None:
    out = seeds(ledger_with_documented_outcome_log(), "C3", "A")

    assert "[2] B" not in out


def toss_up_ledger() -> dict:
    """toss-up: by definition beats_runner_up cannot be written, so the ledger has no such field."""

    return {
        "goal": "Clicking the preview can select the PiP layer",
        "sources": ["claude", "codex"],
        "adjudicator": "claude",
        "disputes": [
            {
                "id": "D1",
                "verdict": "toss-up",
                "question": "Who releases retainVisiblePreview",
                "options": [
                    {
                        "id": "A",
                        "tldr": "the transport owner releases it uniformly",
                        "sources": ["claude"],
                        "cost": "changes the meaning of settleAtPlayhead",
                        "verdict": "chosen",
                        "rank": 1,
                    },
                    {
                        "id": "B",
                        "tldr": "the gesture owner clears it on cancel",
                        "sources": ["codex"],
                        "cost": "adds a new write path",
                        "verdict": "ranked",
                        "rank": 2,
                    },
                ],
                "toss_up_reason": "a trace of one real scrub cancel is missing to separate them",
                "self_selection_note": "the simpler rival B opens a new write path bypassing the existing owner",
                "simpler_absent_check": "Checked, no simpler form",
                "failure_trigger": "after a scrub cancel, clicking the preview still cannot select PiP",
            }
        ],
    }


def test_toss_up_is_a_legal_ledger_without_beats_runner_up() -> None:
    """The definition of toss-up is that the mechanism cannot be written, so record must accept it."""

    from best_of_n import validate

    assert validate(toss_up_ledger()) == []


def test_toss_up_next_gives_an_actionable_switch_instead_of_an_unrecorded_reason() -> None:
    out = seeds(toss_up_ledger(), "D1", "A")

    assert "(not recorded)" not in out


def test_universal_rule_as_written_in_the_skill_is_what_the_script_enforces() -> None:
    """SKILL.md's universal criterion must match what the script actually enforces.

    The original probe asserted, per the SKILL wording of the time, "it is universal as long as every
    source recommended the first place", while the script additionally required it to be the sole
    survivor, so the docs said the same record passed and the script said it failed. The fix was on the
    docs side: if someone else proposed another approach (even ranked second and not disqualified), the
    round is not "everyone gave the same proposal" and should be judged clear or toss-up by whether it
    pulls ahead. Both sides are pinned here so they do not diverge again.
    """

    from best_of_n import validate

    ledger = {
        "goal": "g",
        "sources": ["claude", "codex"],
        "adjudicator": "claude",
        "disputes": [
            {
                "id": "D1",
                "verdict": "universal",
                "question": "q",
                "options": [
                    {
                        "id": "A",
                        "tldr": "every source recommends A",
                        "sources": ["claude", "codex"],
                        "cost": "c",
                        "verdict": "chosen",
                        "rank": 1,
                    },
                    {
                        "id": "B",
                        "tldr": "only one source proposed B",
                        "sources": ["codex"],
                        "cost": "c",
                        "verdict": "ranked",
                        "rank": 2,
                    },
                ],
                "beats_runner_up": "B adds a cross-module write path",
                "self_selection_note": "B bypasses the existing owner",
                "simpler_absent_check": "Checked",
                "failure_trigger": "still reproduces",
            }
        ],
    }

    # There is still a surviving option B, so this is not universal.
    errors = validate(ledger)
    assert any("universal" in e and "surviving" in e for e in errors), errors

    skill = (Path(__file__).resolve().parents[2] / "SKILL.md").read_text(encoding="utf-8")
    assert "the only surviving option" in skill, "SKILL.md's universal criterion does not say \"sole survivor\" and will diverge from the script"

    # Once B is disqualified only one survivor remains, and the same record holds.
    ledger["disputes"][0]["options"][1] = {
        "id": "B",
        "tldr": "only one source proposed B",
        "sources": ["codex"],
        "verdict": "disqualified",
        "disqualified_reason": "fails the locked goal",
    }
    del ledger["disputes"][0]["beats_runner_up"]
    del ledger["disputes"][0]["self_selection_note"]
    assert validate(ledger) == []
