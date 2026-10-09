"""Contracts for loading the review workflows after splitting their entrypoints."""

import re
import unittest
from pathlib import Path


SKILLS = Path(__file__).resolve().parents[4] / "skills"
WORKFLOWS = (
    ("address-review-comments", "github_lane.md"),
    ("review-pr", "full_review.md"),
)


class WorkflowModulesTests(unittest.TestCase):
    def test_entrypoints_fit_cap_and_require_their_helper(self) -> None:
        for skill, helper in WORKFLOWS:
            with self.subTest(skill=skill):
                entrypoint = (SKILLS / skill / "SKILL.md").read_text(encoding="utf-8")
                self.assertLessEqual(len(entrypoint.splitlines()), 250)
                self.assertIn(f"Read(skills/{skill}/scripts/{helper}) completely", entrypoint)

    def test_helpers_load_only_in_their_original_modes(self) -> None:
        author = (SKILLS / "address-review-comments/SKILL.md").read_text(encoding="utf-8")
        reviewer = (SKILLS / "review-pr/SKILL.md").read_text(encoding="utf-8")
        self.assertRegex(
            author,
            r"IF mode == pr AND no --scope AND no explicit --review-file / --free-form-file:\n"
            r"  Read\(skills/address-review-comments/scripts/github_lane.md\) completely",
        )
        self.assertRegex(
            reviewer,
            r"IF MODE == full:\n  Read\(skills/review-pr/scripts/full_review.md\) completely",
        )
        self.assertLess(reviewer.index("1. VERIFICATION BOUNDARY"), reviewer.index("Read(skills/review-pr/scripts/full_review.md)"))
        self.assertIn("6. MODE = incremental", reviewer)
        self.assertIn("CLOSED WORLD:", reviewer)
        self.assertIn("8. DETERMINISTIC RESPONSE GATE", reviewer)

    def test_helper_links_resolve_from_their_new_directory(self) -> None:
        for skill, helper in WORKFLOWS:
            path = SKILLS / skill / "scripts" / helper
            with self.subTest(skill=skill):
                text = path.read_text(encoding="utf-8")
                links = re.findall(r"\[[^\]]+\]\(([^)]+)\)", text)
                self.assertTrue(links)
                for link in links:
                    self.assertTrue((path.parent / link).is_file(), f"{path}: {link}")

    def test_github_helper_keeps_early_returns_and_explicit_input_binding(self) -> None:
        text = (SKILLS / "address-review-comments/scripts/github_lane.md").read_text(encoding="utf-8")
        for status in ("head-moved", "no-comments"):
            self.assertLess(text.index(f"IF `status == {status}`:"), text.index("IF `roundIsCurrentHead == false`"))
        self.assertIn("0 claims, no triage written, no ledger posted", text)
        self.assertIn("IF `cleanReviewAtHead == true`:", text)
        self.assertIn("IF `reviewExists == false`:", text)
        self.assertIn("`<roundDir>/review.md` as the **sole** `--review-file`", text)

    def test_full_review_helper_keeps_probe_and_delegation_boundaries(self) -> None:
        text = (SKILLS / "review-pr/scripts/full_review.md").read_text(encoding="utf-8")
        self.assertIn("Only dimensions 2 / 3 / 7 write and run red probes, restricted to SCOPE_TEST_FILES", text)
        self.assertIn("obey §1 VERIFICATION BOUNDARY", text)
        self.assertIn("**You must also dispatch one fresh-eyes subagent**", text)
        self.assertIn('guide.md "Write behavioral suspicion as a test first, then as a finding"', text)
        self.assertIn("it **must not** read the rest of guide.md", text)
        self.assertIn("The main agent covers the 9 dimensions serially", text)
        self.assertIn("then closes all subagents of this round", text)
        self.assertIn("GOTO REPORT", text)


if __name__ == "__main__":
    unittest.main()
