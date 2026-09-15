"""Mechanical Room Brief ownership contract for the six canonical skills."""

from __future__ import annotations

import os
import shutil
import tempfile
import unittest
from pathlib import Path

from scripts import skill_goal_ownership_check as checker


REPO = Path(__file__).resolve().parents[1]


class LockedRoomBriefContractTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.repo = Path(temporary.name)
        for relative in (checker.GUIDE, checker.CONSUMER):
            target = self.repo / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(REPO / relative, target)
        for name in checker.CANONICAL_SKILLS:
            target = self.repo / "skills" / name / "SKILL.md"
            target.parent.mkdir(parents=True)
            shutil.copy2(REPO / "skills" / name / "SKILL.md", target)
            for registry in checker.DISCOVERY_REGISTRIES:
                link = self.repo / registry / name
                link.parent.mkdir(parents=True, exist_ok=True)
                os.symlink(Path("../../skills") / name, link)

    def skill(self, name: str) -> Path:
        return self.repo / "skills" / name / "SKILL.md"

    def append(self, path: Path, text: str) -> None:
        path.write_text(path.read_text(encoding="utf-8") + text, encoding="utf-8")

    def messages(self) -> list[tuple[str, str]]:
        return [(item.path, item.message) for item in checker.validate_goal_ownership(self.repo)]

    def test_orchestrated_and_standalone_paths_have_one_goal_owner(self) -> None:
        self.assertEqual(checker.validate_goal_ownership(REPO), ())

    def test_copied_contract_passes_before_mutation(self) -> None:
        self.assertEqual(self.messages(), [])

    def test_each_skill_must_route_through_guide_consumer_and_both_branches(self) -> None:
        for name in checker.CANONICAL_SKILLS:
            for required in checker.SKILL_REQUIRED_TEXT:
                with self.subTest(skill=name, required=required):
                    original = self.skill(name).read_text(encoding="utf-8")
                    self.skill(name).write_text(original.replace(required, "removed"), encoding="utf-8")
                    self.assertIn(
                        (f"skills/{name}/SKILL.md", f"skill does not route through `{required}`"),
                        self.messages(),
                    )
                    self.skill(name).write_text(original, encoding="utf-8")

    def test_skill_must_not_duplicate_guide_authority_or_claim_brief_writing(self) -> None:
        self.append(self.skill("review-plan"), "\nRead BUS_TRUSTED_ASSIGNMENT_TOKEN.\n")
        self.append(self.skill("create-plan"), "\nCall ProposeRoomBrief.\n")
        messages = self.messages()
        self.assertIn(
            ("skills/review-plan/SKILL.md", "skill duplicates guide authority `BUS_TRUSTED_ASSIGNMENT_TOKEN`"),
            messages,
        )
        self.assertIn(
            ("skills/create-plan/SKILL.md", "coding-agent skill claims Room Brief proposal authority"),
            messages,
        )

    def test_review_locks_have_a_single_writer(self) -> None:
        self.append(self.skill("review-pr"), "\nwrite the reply to temp/review-pr/x/.locked-goal\n")
        self.assertIn(
            ("skills/review-pr/SKILL.md", "review locks are written outside pr_goal_context.py"),
            self.messages(),
        )

    def test_verifier_absence_never_maps_to_standalone(self) -> None:
        self.append(self.skill("pr"), "\nIF verifier absent: continue standalone\n")
        self.assertIn(
            ("skills/pr/SKILL.md", "skill maps a verifier absence to standalone"),
            self.messages(),
        )

    def test_entrypoint_length_and_severity_labels_are_enforced(self) -> None:
        self.append(self.skill("execute-plan"), "\n" * checker.ENTRYPOINT_MAX_LINES)
        self.append(self.skill("address-review-comments"), "\n### [P1] Broken routing\n")
        messages = self.messages()
        self.assertIn(("skills/execute-plan/SKILL.md", "entrypoint exceeds 250 lines"), messages)
        self.assertIn(
            ("skills/address-review-comments/SKILL.md", "skill contains review severity labels"),
            messages,
        )

    def test_guide_must_define_branches_and_handoff(self) -> None:
        guide = self.repo / checker.GUIDE
        guide.write_text(
            guide.read_text(encoding="utf-8")
            .replace("## Participant handoff", "## Notes")
            .replace("NotInBusRoom", "outside"),
            encoding="utf-8",
        )
        messages = self.messages()
        self.assertIn((checker.GUIDE, "guide lacks heading `## Participant handoff`"), messages)
        self.assertIn((checker.GUIDE, "guide does not state `NotInBusRoom`"), messages)

    def test_no_second_trust_validator_or_discovery_reader_exists(self) -> None:
        helper = self.repo / "skills" / "pr" / "scripts" / "probe.py"
        helper.parent.mkdir(parents=True)
        helper.write_text("TOKEN = open('.read-token')\nDIR = 'BUS_TRUSTED_ASSIGNMENT_DIR'\n", encoding="utf-8")
        (self.repo / checker.CONSUMER).unlink()
        messages = self.messages()
        self.assertIn(
            ("skills/pr/scripts/probe.py", "helper reads trusted assignment storage marker `.read-token`"),
            messages,
        )
        self.assertIn(
            ("skills/pr/scripts/probe.py", "only the shared consumer may read `BUS_TRUSTED_ASSIGNMENT_DIR`"),
            messages,
        )
        self.assertIn((checker.CONSUMER, "shared room assignment consumer is missing"), messages)

    def test_discovery_links_and_orchestrator_only_skills(self) -> None:
        link = self.repo / ".claude" / "skills" / "pr"
        link.unlink()
        link.mkdir()
        (self.repo / "skills" / "execute-workflow").mkdir()
        messages = self.messages()
        self.assertIn((".claude/skills/pr", "discovery link does not resolve to the canonical skill"), messages)
        self.assertIn(("skills/execute-workflow", "orchestrator-only skill is exposed to coding agents"), messages)


if __name__ == "__main__":
    unittest.main()
