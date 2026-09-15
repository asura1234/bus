"""Objective production content checks; no model, provider, or driver is executed."""

from __future__ import annotations

import json
import shutil
import tempfile
import unittest
from pathlib import Path

from scripts import orchestrator_content_check as checker


REPO = Path(__file__).resolve().parents[1]
MERMAID_BLOCK = "```mermaid\nflowchart TD\n    A --> B\n```\n"


class CheckedInContentContractTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.repo = Path(temporary.name)
        shutil.copytree(REPO / checker.PRODUCTION, self.repo / checker.PRODUCTION)
        for name in checker.CANONICAL_SKILLS:
            target = self.repo / "skills" / name / "SKILL.md"
            target.parent.mkdir(parents=True)
            shutil.copy2(REPO / "skills" / name / "SKILL.md", target)
        guide = self.repo / checker.ROOM_BRIEF_GUIDE
        guide.parent.mkdir(parents=True)
        shutil.copy2(REPO / checker.ROOM_BRIEF_GUIDE, guide)

    def production(self, name: str) -> Path:
        return self.repo / checker.PRODUCTION / name

    def edit_manifest(self, change) -> None:
        path = self.repo / checker.MANIFEST
        manifest = json.loads(path.read_text(encoding="utf-8"))
        change(manifest)
        path.write_text(json.dumps(manifest), encoding="utf-8")

    def messages(self) -> list[tuple[str, str]]:
        return [(item.path, item.message) for item in checker.validate_repository(self.repo)]

    def test_checked_in_production_tree_passes_objective_checks(self) -> None:
        self.assertEqual(checker.validate_repository(REPO), ())

    def test_copied_tree_passes_before_mutation(self) -> None:
        self.assertEqual(self.messages(), [])

    def test_missing_required_entry_is_reported(self) -> None:
        self.edit_manifest(
            lambda manifest: manifest.__setitem__(
                "references",
                [entry for entry in manifest["references"] if entry["name"] != "sop-review-pr"],
            )
        )
        self.assertIn(
            ("src/bus/orchestrator/content/production/manifest.json", "missing required reference entry sop-review-pr"),
            self.messages(),
        )

    def test_nested_or_missing_entry_file_is_reported(self) -> None:
        def change(manifest: dict) -> None:
            manifest["skills"][0]["path"] = "skills/create-workflow.md"
            manifest["agent"]["path"] = "missing-agent.md"

        self.edit_manifest(change)
        messages = [message for _, message in self.messages()]
        self.assertIn("skill/create-workflow path must be one flat .md file", messages)
        self.assertIn("agent file missing-agent.md is missing", messages)

    def test_unreadable_manifest_is_reported(self) -> None:
        (self.repo / checker.MANIFEST).write_text("[", encoding="utf-8")
        self.assertIn(
            ("src/bus/orchestrator/content/production/manifest.json", "manifest is missing or not a JSON object"),
            self.messages(),
        )

    def test_missing_template_section_is_reported(self) -> None:
        template = self.production("workflow-template.md")
        template.write_text(
            template.read_text(encoding="utf-8").replace("## Human tasks\n", "## Human work\n"),
            encoding="utf-8",
        )
        self.assertIn(
            ("src/bus/orchestrator/content/production/workflow-template.md", "missing required section `## Human tasks`"),
            self.messages(),
        )

    def test_zero_or_two_mermaid_flowcharts_are_reported(self) -> None:
        review_plan = self.production("sop-review-plan.md")
        text = review_plan.read_text(encoding="utf-8")
        start = text.index("```mermaid")
        end = text.index("```", start + len("```mermaid")) + len("```")
        review_plan.write_text(text[:start] + text[end:], encoding="utf-8")
        review_pr = self.production("sop-review-pr.md")
        review_pr.write_text(review_pr.read_text(encoding="utf-8") + "\n" + MERMAID_BLOCK, encoding="utf-8")
        template = self.production("workflow-template.md")
        template.write_text(template.read_text(encoding="utf-8") + "\n" + MERMAID_BLOCK, encoding="utf-8")

        messages = self.messages()
        prefix = "src/bus/orchestrator/content/production/"
        self.assertIn((prefix + "sop-review-plan.md", "expected exactly one Mermaid flowchart, found 0"), messages)
        self.assertIn((prefix + "sop-review-pr.md", "expected exactly one Mermaid flowchart, found 2"), messages)
        self.assertIn((prefix + "workflow-template.md", "expected exactly one Mermaid flowchart, found 2"), messages)

    def test_non_flowchart_mermaid_block_does_not_count(self) -> None:
        self.assertEqual(checker.mermaid_flowchart_count("```mermaid\nsequenceDiagram\n```\n"), 0)
        self.assertEqual(checker.mermaid_flowchart_count("```text\nflowchart TD\n```\n"), 0)
        self.assertEqual(checker.mermaid_flowchart_count(MERMAID_BLOCK), 1)

    def test_removed_capability_tokens_are_rejected_in_both_forms(self) -> None:
        execute = self.production("execute-workflow.md")
        execute.write_text(execute.read_text(encoding="utf-8") + "\nUse `ReadArtifact`.\n", encoding="utf-8")
        skill = self.repo / "skills" / "pr" / "SKILL.md"
        skill.write_text(skill.read_text(encoding="utf-8") + "\ncall request_human\n", encoding="utf-8")
        guide = self.repo / checker.ROOM_BRIEF_GUIDE
        guide.write_text(guide.read_text(encoding="utf-8") + "\nSteerActiveWork\n", encoding="utf-8")

        messages = self.messages()
        self.assertIn(
            ("src/bus/orchestrator/content/production/execute-workflow.md", "removed capability token `ReadArtifact`"),
            messages,
        )
        self.assertIn(("skills/pr/SKILL.md", "removed capability token `request_human`"), messages)
        self.assertIn(
            ("docs/guides/orchestrated-room-brief.md", "removed capability token `SteerActiveWork`"),
            messages,
        )

    def test_closed_surface_names_and_longer_words_are_not_removed_tokens(self) -> None:
        execute = self.production("execute-workflow.md")
        execute.write_text(
            execute.read_text(encoding="utf-8")
            + "\n`ReadContent` `read_content` ReadArtifactory my_read_artifact_notes\n",
            encoding="utf-8",
        )
        self.assertEqual(self.messages(), [])

    def test_missing_scanned_file_is_reported(self) -> None:
        (self.repo / checker.ROOM_BRIEF_GUIDE).unlink()
        self.assertIn(("docs/guides/orchestrated-room-brief.md", "scanned file is missing"), self.messages())


if __name__ == "__main__":
    unittest.main()
