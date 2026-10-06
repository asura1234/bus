"""The prologue part shared by `/review-pr` and `/review-plan`.

The reason for extracting it is not "a few fewer lines" but that drift here fails **silently**: a wrong
lane ownership raises no error, it only makes some reviewer read someone else's PREV_REVIEWS, so
cross-round reconciliation is done against the wrong history; wrong ledger discovery lets
already-rejected findings reappear, or conversely suppresses code findings with plan-mode triage. An
error with no red light must be prevented by a single implementation, not by keeping two copies in sync.
"""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "cli_extensions"))

import review_round_common as common  # noqa: E402
from review_round_common import (  # noqa: E402
    bare_call_ambiguous_lanes,
    claim_bare_round,
    lanes_with_history,
    triage_mode,
)


def _finished_round(lane_root: Path, lane: str, number: int) -> None:
    d = lane_root / lane / f"round-{number:02d}"
    d.mkdir(parents=True)
    (d / "review.md").write_text("# Review\n")


class ReviewRoundCommonTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.tmp_path = Path(temporary.name)

    def test_solo_bare_calls_accumulate_in_one_lane(self) -> None:
        lane, target, completed = claim_bare_round(self.tmp_path)
        self.assertEqual((lane, completed), ("default", []))
        (target / "review.md").write_text("# Review\n")

        lane, target, completed = claim_bare_round(self.tmp_path)
        self.assertTrue(lane == "default" and len(completed) == 1)
        self.assertEqual(target.name, "round-02")

    def test_an_inflight_round_bumps_to_the_next_lane_instead_of_overwriting(self) -> None:
        """Concurrent bare calls must each leave a trace — claiming relies on the atomicity of mkdir(exist_ok=False)."""
        first_lane, first_target, _ = claim_bare_round(self.tmp_path)
        self.assertEqual(first_lane, "default")  # In flight: no review.md

        second_lane, second_target, _ = claim_bare_round(self.tmp_path)

        self.assertEqual(second_lane, "default-2")
        self.assertNotEqual(second_target, first_target)

    def test_a_named_lane_makes_bare_calls_ambiguous(self) -> None:
        _finished_round(self.tmp_path, "default", 1)
        _finished_round(self.tmp_path, "bus-claude", 1)

        self.assertEqual(lanes_with_history(self.tmp_path), ["bus-claude", "default"])
        # With a named lane present a bare call cannot be attributed: silently falling back to default
        # would lose cross-round ownership.
        self.assertEqual(bare_call_ambiguous_lanes(["bus-claude", "default"]), ["bus-claude", "default"])
        # A single default-family lane can still continue automatically.
        self.assertEqual(bare_call_ambiguous_lanes(["default"]), [])
        self.assertEqual(bare_call_ambiguous_lanes(["default-2"]), [])
        # Two concurrent first rounds each finished; continuing either reads someone else's history.
        self.assertEqual(bare_call_ambiguous_lanes(["default", "default-2"]), ["default", "default-2"])

    def test_triage_mode_is_read_from_the_declaration_and_fails_closed(self) -> None:
        pr = self.tmp_path / "pr.md"
        pr.write_text("**模式**：pr\n\n## APPLY\n")
        self.assertEqual(triage_mode(pr), "pr")

        plan = self.tmp_path / "plan.md"
        plan.write_text("**模式**：plan\n")
        self.assertEqual(triage_mode(plan), "plan")

        unknown = self.tmp_path / "unknown.md"
        unknown.write_text("没有声明 mode\n")
        self.assertIsNone(triage_mode(unknown))

        self.assertIsNone(triage_mode(self.tmp_path / "missing.md"))

    def test_triage_ledgers_filters_by_the_caller_supplied_mode(self) -> None:
        """mode must be passed explicitly: suppressing code findings with plan triage silently buries real problems."""
        root = self.tmp_path / "temp" / "address-review-comments" / "feat-x"
        for name, mode in (("a", "pr"), ("b", "plan"), ("c", "pr")):
            d = root / name
            d.mkdir(parents=True)
            (d / "triage.md").write_text(f"**模式**：{mode}\n")

        task = self.tmp_path / "temp" / "address-review-comments" / "__task__" / "t"
        task.mkdir(parents=True)
        (task / "triage.md").write_text("**模式**：pr\n")

        with mock.patch.object(common, "REPO_ROOT", self.tmp_path):
            pr_names = sorted(p.parent.name for p in common.triage_ledgers("feat-x", "pr"))
            plan_names = sorted(p.parent.name for p in common.triage_ledgers("feat-x", "plan"))

            self.assertEqual(pr_names, ["a", "c"])
            self.assertEqual(plan_names, ["b"])
            self.assertEqual(common.triage_ledgers("no-such-branch", "pr"), [])


if __name__ == "__main__":
    unittest.main()
