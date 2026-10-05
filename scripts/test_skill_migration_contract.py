import importlib.util
import sys
import unittest
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]
SKILLS = REPO / "skills"
CANONICAL_SKILLS = {
    "address-review-comments",
    "commit-and-push",
    "create-plan",
    "delete-dead-code",
    "execute-plan",
    "gate-and-fix",
    "pr",
    "rebase-origin-main",
    "review-plan",
    "review-pr",
    "split-pr",
    "update-docs",
    "worktree-close",
    "worktree-new",
}
PRODUCT_ONLY_SKILLS = {
    "desktop-app-testing",
    "desktop-dev-cases",
    "network-devtool",
}


def _review_types_module():
    path = REPO / "cli_extensions" / "review_artifact_types.py"
    spec = importlib.util.spec_from_file_location("review_artifact_types", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class SkillMigrationContractTest(unittest.TestCase):
    def test_canonical_workflow_inventory_and_entrypoints(self) -> None:
        actual = {
            entry.name
            for entry in SKILLS.iterdir()
            if entry.is_dir() and (entry / "SKILL.md").is_file()
        }
        self.assertTrue(CANONICAL_SKILLS.issubset(actual))
        self.assertTrue(PRODUCT_ONLY_SKILLS.isdisjoint(actual))
        for name in CANONICAL_SKILLS:
            with self.subTest(skill=name):
                entrypoint = SKILLS / name / "SKILL.md"
                self.assertLessEqual(len(entrypoint.read_text().splitlines()), 250)

    def test_agent_registries_are_derived_links(self) -> None:
        for registry in (REPO / ".agents" / "skills", REPO / ".claude" / "skills"):
            for name in CANONICAL_SKILLS:
                with self.subTest(registry=registry, skill=name):
                    link = registry / name
                    self.assertTrue(link.is_symlink())
                    self.assertEqual(link.readlink(), Path("../../skills") / name)
                    self.assertTrue((link / "SKILL.md").is_file())

    def test_shared_workflow_sources_exist(self) -> None:
        required = (
            "docs/guides/review-format.md",
            "docs/guides/plan-review-guide.md",
            "docs/guides/code-review-guide.md",
            "docs/guides/review-response-guide.md",
            "docs/guides/task-review-guide.md",
            "docs/guides/plan-execution-guide.md",
            "docs/guides/task-agent-report-format.md",
            "docs/guides/execute-plan-action-format.md",
            "docs/templates/plan-template.md",
            "docs/templates/module-agents-template.md",
            "cli_extensions/review_artifact.py",
            "cli_extensions/review_artifact_parser.py",
            "cli_extensions/review_artifact_task.py",
            "cli_extensions/review_artifact_types.py",
        )
        for relative in required:
            with self.subTest(path=relative):
                self.assertTrue((REPO / relative).is_file())

    def test_review_verdicts_are_the_canonical_closed_sets(self) -> None:
        verdicts = _review_types_module().LEGAL_VERDICTS
        self.assertEqual(
            verdicts["plan"],
            {"可执行（Ready）", "需要完善（Needs Refinement）", "废弃（Abandon）"},
        )
        self.assertEqual(verdicts["pr"], {"Ready", "Needs Refinement", "Abandon"})
        self.assertEqual(
            verdicts["task"], {"Ready", "Needs Refinement", "Plan Repair Required"}
        )
        markdown = "\n".join(
            path.read_text(encoding="utf-8")
            for root in (SKILLS, REPO / "docs")
            for path in root.rglob("*.md")
        )
        self.assertNotIn("Not Ready", markdown)

    def test_bus_repository_adapters_are_in_the_templates(self) -> None:
        plan = (REPO / "docs/templates/plan-template.md").read_text()
        self.assertIn("1. `just lint`\n2. `just test`\n3. `just build`", plan)
        self.assertNotIn("./run test coverage", plan)

        pr = (REPO / "skills/pr/references/pr-template.md").read_text()
        self.assertIn("`just test`", pr)
        self.assertIn("`just lint`", pr)
        self.assertNotIn("docs/plans/", pr)

    def test_template_and_format_inventory_is_complete(self) -> None:
        expected = {
            "docs/guides/consumer-fallout-format.md",
            "docs/guides/execute-plan-action-format.md",
            "docs/guides/review-format.md",
            "docs/guides/task-agent-report-format.md",
            "docs/templates/module-agents-template.md",
            "docs/templates/plan-template.md",
            "skills/delete-dead-code/references/dead-code-findings-format.md",
            "skills/gate-and-fix/references/gate-round-format.md",
            "skills/pr/references/pr-template.md",
            "skills/update-docs/references/docs-audit-format.md",
        }
        actual = {
            path.relative_to(REPO).as_posix()
            for root in (REPO / "docs", REPO / "skills")
            for path in root.rglob("*.md")
            if path.name.endswith(("template.md", "format.md"))
        }
        self.assertEqual(actual, expected)

        combined = "\n".join((REPO / path).read_text() for path in sorted(expected))
        for stale in (
            "App Server",
            "cmake/Coverage.cmake",
            "docs/architecture/process-environment.md",
            "../../AGENTS.md",
            "shell/packages/",
            "LibTV PR 模板",
        ):
            with self.subTest(stale=stale):
                self.assertNotIn(stale, combined)


if __name__ == "__main__":
    unittest.main()
