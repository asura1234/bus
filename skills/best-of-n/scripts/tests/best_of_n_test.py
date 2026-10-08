"""Fail-closed contract of best-of-n records.

Each case corresponds to one "looks done, but no decision was actually made" shape. The most important
is that options must not be lost: the point of the ranking is a fallback when the first place fails,
and an option deleted at adjudication time is one nobody will discover ever existed, weeks later.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest


SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

from best_of_n import (  # noqa: E402
    LedgerError,
    main,
    record_attempt,
    render,
    seeds,
    validate,
)


def ledger(**overrides):
    base = {
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
                "self_selection_note": "the simpler rival B opens a new write path bypassing the existing owner, while A only lifts an existing check",
                "simpler_absent_check": "Checked, no simpler form",
                "failure_trigger": "after a scrub cancel, clicking the preview still cannot select PiP",
            }
        ],
    }
    base.update(overrides)
    return base


def test_well_formed_ledger_passes():
    assert validate(ledger()) == []


def test_two_chosen_is_not_a_decision():
    data = ledger()
    data["disputes"][0]["options"][1]["verdict"] = "chosen"
    data["disputes"][0]["options"][1]["rank"] = 1
    errors = validate(data)
    assert any("exactly one chosen" in e for e in errors)


def test_no_chosen_is_not_a_decision():
    data = ledger()
    data["disputes"][0]["options"][0]["verdict"] = "ranked"
    assert any("exactly one chosen" in e for e in validate(data))


def test_tied_ranks_are_rejected():
    data = ledger()
    data["disputes"][0]["options"][1]["rank"] = 1
    errors = validate(data)
    assert any("permutation" in e for e in errors)


def test_rank_gap_is_rejected():
    data = ledger()
    data["disputes"][0]["options"][1]["rank"] = 3
    assert any("permutation" in e for e in validate(data))


def test_chosen_must_be_rank_one():
    data = ledger()
    data["disputes"][0]["options"][0]["rank"] = 2
    data["disputes"][0]["options"][1]["rank"] = 1
    assert any("rank must be 1" in e for e in validate(data))


def test_disqualified_needs_a_reason_and_no_rank():
    data = ledger()
    data["disputes"][0]["options"][1] = {
        "id": "B",
        "tldr": "record it as a known limitation",
        "sources": ["cursor"],
        "verdict": "disqualified",
        "rank": 2,
    }
    errors = validate(data)
    assert any("a disqualification must state" in e for e in errors)
    assert any("must not have a rank" in e for e in errors)


def test_option_without_tldr_is_rejected():
    """An option without a TLDR is unusable when switching — which is exactly why the ranking exists."""
    data = ledger()
    data["disputes"][0]["options"][1]["tldr"] = ""
    assert any("missing tldr" in e for e in validate(data))


def test_failure_trigger_is_required():
    data = ledger()
    del data["disputes"][0]["failure_trigger"]
    assert any("failure_trigger" in e for e in validate(data))


def test_beats_runner_up_required_when_two_survive():
    data = ledger()
    del data["disputes"][0]["beats_runner_up"]
    assert any("why the first place beats the second" in e for e in validate(data))


def test_single_survivor_needs_no_runner_up_reason():
    data = ledger()
    only = data["disputes"][0]["options"][0]
    # With only one option left, the other participants must be merged into this option's sources —
    # otherwise they become "proposed nothing", which usually means their replies were never normalized in.
    only["sources"] = sorted(set(only["sources"]) | set(data["sources"]))
    data["disputes"][0]["options"] = [only]
    del data["disputes"][0]["beats_runner_up"]
    del data["disputes"][0]["self_selection_note"]
    assert validate(data) == []


def test_missing_goal_is_rejected():
    data = ledger()
    del data["goal"]
    assert any("missing goal" in e for e in validate(data))


def test_seeds_returns_every_survivor_not_just_the_next_one():
    """The remaining options are the seeds of the second round; compressing them into "pop the head" loses the new evidence the failure brings."""
    out = seeds(ledger(), "C3", "A")
    assert "REMAINING=1" in out
    assert "the gesture owner clears it on cancel" in out
    assert "seeds of the second round" in out
    assert "Why the first place won back then" in out


def test_seeds_carries_the_failure_criterion_and_history():
    data = record_attempt(ledger(), "C3", "A", "still reproduces after the scrub cancel change")
    out = seeds(data, "C3", "A")
    assert "Failure criterion" in out
    assert "A: still reproduces after the scrub cancel change" in out


def test_seeds_excludes_everything_already_attempted():
    """It is not only the one just passed in that failed — everything already tried must not become a seed again."""
    data = record_attempt(ledger(), "C3", "A", "still reproduces")
    out = seeds(data, "C3", "B")
    assert "EXHAUSTED" in out
    assert "Do not casually invent" in out


def test_seeds_rejects_unknown_dispute_and_option():
    with pytest.raises(LedgerError):
        seeds(ledger(), "NOPE", "A")
    with pytest.raises(LedgerError):
        seeds(ledger(), "C3", "Z")


def test_record_attempt_rejects_unknown_option_and_empty_outcome():
    with pytest.raises(LedgerError):
        record_attempt(ledger(), "C3", "Z", "x")
    with pytest.raises(LedgerError):
        record_attempt(ledger(), "C3", "A", "   ")


def test_missing_sources_or_adjudicator_is_rejected():
    data = ledger()
    del data["sources"]
    assert any("missing sources" in e for e in validate(data))
    data = ledger()
    del data["adjudicator"]
    assert any("missing adjudicator" in e for e in validate(data))


def test_self_selection_needs_a_named_mechanism():
    """The adjudicator choosing their own approach is the default state, not an exception — but where the rival loses must be written down."""
    data = ledger()
    del data["disputes"][0]["self_selection_note"]
    assert any("self_selection_note" in e for e in validate(data))


def test_self_selection_note_not_required_when_adjudicator_did_not_propose_winner():
    data = ledger()
    data["adjudicator"] = "codex"
    del data["disputes"][0]["self_selection_note"]
    assert validate(data) == []


def test_simpler_absent_check_is_required():
    """An option nobody proposed cannot win — that is exactly what unanimous agreement hides."""
    data = ledger()
    del data["disputes"][0]["simpler_absent_check"]
    assert any("simpler_absent_check" in e for e in validate(data))


def test_option_source_outside_the_round_is_rejected():
    data = ledger()
    data["disputes"][0]["options"][0]["sources"] = ["gemini"]
    assert any("not in this round's sources" in e for e in validate(data))


def test_self_selection_line_only_renders_when_it_actually_happened():
    """When the adjudicator chose someone else's approach, this line would state something that did not happen."""
    data = ledger()
    data["adjudicator"] = "codex"  # codex proposed B; A was chosen
    out = render(data)
    assert "The adjudicator chose their own approach" not in out
    assert "The adjudicator chose their own approach" in render(ledger())  # claude proposed A and chose A


def test_unanimity_is_surfaced_in_the_render():
    data = ledger()
    data["disputes"][0]["options"] = [data["disputes"][0]["options"][0]]
    data["disputes"][0]["verdict"] = "universal"
    data["disputes"][0]["options"] = [data["disputes"][0]["options"][0]]
    data["disputes"][0]["options"][0]["sources"] = ["claude", "codex", "cursor"]
    del data["disputes"][0]["beats_runner_up"]
    assert validate(data) == []
    assert "unanimously recommended A" in render(data)


def test_render_lists_every_option_and_marks_the_winner():
    out = render(ledger())
    assert "**1 ✓**" in out
    assert "the transport owner releases it uniformly" in out
    assert "the gesture owner clears it on cancel" in out


def test_record_writes_only_on_pass(tmp_path: Path):
    good, out = tmp_path / "g.json", tmp_path / "g.md"
    good.write_text(json.dumps(ledger(), ensure_ascii=False))
    assert main(["record", "--ledger", str(good), "--output", str(out)]) == 0
    assert out.is_file()

    bad, bad_out = tmp_path / "b.json", tmp_path / "b.md"
    broken = ledger()
    broken["disputes"][0]["options"][1]["rank"] = 1
    bad.write_text(json.dumps(broken, ensure_ascii=False))
    assert main(["record", "--ledger", str(bad), "--output", str(bad_out)]) == 1
    assert not bad_out.exists()


def test_malformed_ledger_exits_two(tmp_path: Path):
    path = tmp_path / "x.json"
    path.write_text("not json")
    assert main(["record", "--ledger", str(path), "--output", str(tmp_path / "o.md")]) == 2


def test_verdict_is_required_and_checked():
    data = ledger()
    del data["disputes"][0]["verdict"]
    assert any("verdict must be" in e for e in validate(data))


def test_clear_without_a_named_mechanism_is_rejected():
    """If no mechanism can be written it did not actually pull ahead of the second place — that is toss-up, not clear."""
    data = ledger()
    del data["disputes"][0]["beats_runner_up"]
    assert any("judging clear" in e for e in validate(data))


def test_toss_up_needs_what_would_break_the_tie():
    data = ledger()
    data["disputes"][0]["verdict"] = "toss-up"
    del data["disputes"][0]["beats_runner_up"]
    errors = validate(data)
    assert any("toss_up_reason" in e for e in errors)


def test_toss_up_keeps_its_ranking_and_needs_no_justification():
    """The ranking may be arbitrary — its value is giving a next action, not proving which is better."""
    data = ledger()
    data["disputes"][0]["verdict"] = "toss-up"
    del data["disputes"][0]["beats_runner_up"]
    data["disputes"][0]["toss_up_reason"] = "neither can be falsified without running one real scrub"
    assert validate(data) == []
    out = render(data)
    assert "toss-up" in out
    assert "the next step is still to do A first" in out


def test_universal_must_actually_be_unanimous():
    data = ledger()
    data["disputes"][0]["verdict"] = "universal"
    assert any("not independently recommended by every source" in e for e in validate(data))


def test_adjudicator_outside_the_round_is_rejected():
    """An outside identity matches no option's sources and silently disables the self-selection gate."""
    data = ledger()
    data["adjudicator"] = "outsider"
    errors = validate(data)
    assert any("not in this round's sources" in e for e in errors)


def test_universal_requires_a_sole_survivor():
    """If someone proposed something else, it is not "everyone gave the same proposal"."""
    data = ledger()
    data["disputes"][0]["verdict"] = "universal"
    data["disputes"][0]["options"][0]["sources"] = ["claude", "codex", "cursor"]
    errors = validate(data)
    assert any("still 1 other surviving option" in e for e in errors)
