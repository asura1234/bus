"""validate must report every independent defect at once, not one at a time.

`record`'s contract is "FAIL + every reason; fix once and it passes". The early exit added for "options
contains non-object entries" was once written as `if errors: return errors`, so any earlier unrelated
dispute-level error (missing question, invalid verdict) skipped the whole option-level validation along
with _consensus_errors — a record missing question and with two options tied at rank 1 reported only the
former, and the adjudicator had to run it N times to learn what else was missing.
"""

from __future__ import annotations

import sys
from pathlib import Path


SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

from best_of_n import validate  # noqa: E402


def _ledger() -> dict:
    return {
        "goal": "g",
        "sources": ["a", "b"],
        "adjudicator": "a",
        "disputes": [
            {
                "id": "D1",
                "verdict": "clear",
                "options": [
                    {"id": "A", "tldr": "t", "sources": ["a"], "cost": "c", "verdict": "chosen", "rank": 1},
                    {"id": "B", "tldr": "t", "sources": ["b"], "cost": "c", "verdict": "ranked", "rank": 1},
                ],
                "beats_runner_up": "x",
                "simpler_absent_check": "y",
                "failure_trigger": "z",
            }
        ],
    }


def test_an_unrelated_dispute_level_defect_does_not_hide_the_option_level_ones() -> None:
    errors = validate(_ledger())  # missing question + tied rank 1

    assert any("question" in e for e in errors), errors
    assert any("rank" in e for e in errors), f"the tied rank 1 was hidden by the unrelated missing question: {errors}"


def test_a_malformed_option_still_short_circuits_the_option_level_checks() -> None:
    """Non-object entries must still exit early — every later `.get` assumes an object."""

    broken = _ledger()
    broken["disputes"][0]["options"] = ["A"]

    errors = validate(broken)

    assert any("not an object" in e for e in errors), errors
