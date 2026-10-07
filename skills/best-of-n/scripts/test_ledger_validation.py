"""Invariants of ledger validation, rendering, and next: options must not be lost, and a conclusion must not be disguised as agreement."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest


SCRIPTS_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS_DIR))

from best_of_n import (  # noqa: E402
    main,
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


def test_skill_points_at_the_subcommand_that_actually_writes_the_log(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """The switch history must be pointed by SKILL at the subcommand that actually writes the ledger.

    The original probe caught that §0 told people to "rerun record" to append the failure, while
    `record` only validates and renders Markdown and never touches the ledger — so the failure was never
    persisted, and the next `next` recommended an option that had already failed as a seed again. The
    fix pointed the docs at `attempt`. Both sides are pinned here: the docs must name attempt rather than
    record, and after following them the failed option really no longer appears among the seeds.
    """
    skill = (Path(__file__).resolve().parents[1] / "SKILL.md").read_text(encoding="utf-8")
    switch = skill[skill.index("Overturned") : skill.index("Overturned") + 400]
    assert "attempt" in switch, f"the §0 switch path does not point at attempt: {switch[:200]}"

    path = tmp_path / "ledger.json"
    rendered = tmp_path / "decisions.md"
    path.write_text(json.dumps(ledger(), ensure_ascii=False), encoding="utf-8")

    # record does not write the ledger — still true, just no longer the documented way to switch.
    assert main(["record", "--ledger", str(path), "--output", str(rendered)]) == 0
    assert json.loads(path.read_text(encoding="utf-8"))["disputes"][0].get("outcome_log", []) == []

    # Following the docs with attempt is what actually persists the failure.
    assert main(["attempt", "--ledger", str(path), "--dispute", "C3", "--option", "A", "--outcome", "still reproduces"]) == 0
    capsys.readouterr()
    assert main(["next", "--ledger", str(path), "--dispute", "C3", "--failed", "B"]) == 0
    out = capsys.readouterr().out
    assert "the transport owner releases it uniformly" not in out, out


def test_attempt_then_next_excludes_every_recorded_failure(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    path = tmp_path / "ledger.json"
    path.write_text(json.dumps(ledger(), ensure_ascii=False), encoding="utf-8")
    assert (
        main(
            [
                "attempt",
                "--ledger",
                str(path),
                "--dispute",
                "C3",
                "--option",
                "A",
                "--outcome",
                "still reproduces after the change",
            ]
        )
        == 0
    )
    assert main(["next", "--ledger", str(path), "--dispute", "C3", "--failed", "B"]) == 0
    out = capsys.readouterr().out
    assert "EXHAUSTED" in out
    assert "the transport owner releases it uniformly" not in out


def test_documented_outcome_log_format_is_recognized_as_attempted() -> None:
    """The outcome_log example in the module docstring / --help schema is "2026-09-16 A failed: ...".

    seeds splits on the first colon and must recognize that A was already tried; otherwise a second next would offer A as a seed again.
    """
    data = ledger()
    data["disputes"][0]["outcome_log"] = [
        "2026-09-16 A failed: still reproduces after the scrub cancel change, switching to B",
    ]
    out = seeds(data, "C3", "B")
    assert "EXHAUSTED" in out
    assert "the transport owner releases it uniformly" not in out


def test_participating_source_with_no_option_is_rejected() -> None:
    """"Options must not be lost": every participant in sources must appear on at least one option."""
    data = ledger()
    data["disputes"][0]["options"][0]["sources"] = ["claude"]
    data["disputes"][0]["options"][1]["sources"] = ["codex"]
    errors = validate(data)
    assert any("cursor" in e and ("dropping" in e or "option" in e or "source" in e) for e in errors)


def test_universal_with_only_disqualified_runner_up_is_unanimous() -> None:
    data = ledger()
    data["disputes"][0]["verdict"] = "universal"
    data["disputes"][0]["options"][0]["sources"] = ["claude", "codex", "cursor"]
    data["disputes"][0]["options"][1] = {
        "id": "B",
        "tldr": "record it as a known limitation",
        "sources": ["codex"],
        "verdict": "disqualified",
        "disqualified_reason": "fails the locked goal",
    }
    del data["disputes"][0]["beats_runner_up"]
    del data["disputes"][0]["self_selection_note"]
    assert validate(data) == []


def test_toss_up_with_a_single_survivor_is_rejected() -> None:
    data = ledger()
    data["disputes"][0]["verdict"] = "toss-up"
    data["disputes"][0]["options"] = [data["disputes"][0]["options"][0]]
    data["disputes"][0]["toss_up_reason"] = "one real scrub is still missing"
    del data["disputes"][0]["beats_runner_up"]
    del data["disputes"][0]["self_selection_note"]
    errors = validate(data)
    assert any("toss-up" in e and "two" in e for e in errors)


def test_render_dumps_option_fields_verbatim() -> None:
    data = ledger()
    data["disputes"][0]["options"][0]["tldr"] = "token=ghp_example_secret_value"
    out = render(data)
    assert "token=ghp_example_secret_value" in out


def test_next_cli_on_valid_ledger_lists_remaining(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    path = tmp_path / "ledger.json"
    path.write_text(json.dumps(ledger(), ensure_ascii=False), encoding="utf-8")
    assert main(["next", "--ledger", str(path), "--dispute", "C3", "--failed", "A"]) == 0
    captured = capsys.readouterr()
    assert "REMAINING=1" in captured.out
    assert "the gesture owner clears it on cancel" in captured.out
