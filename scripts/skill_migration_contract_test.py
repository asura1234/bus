import importlib.util
import sys
import unittest
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]
SKILLS = REPO / "skills"
CANONICAL_SKILLS = {
    "address-review-comments",
    "best-of-n",
    "commit-and-push",
    "delete-dead-code",
    "e2e-test",
    "gate-and-fix",
    "merge-pr",
    "pr",
    "rebase-origin-main",
    "review-plan",
    "review-pr",
    "split-pr",
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
    def test_generic_gate_and_rebase_mechanics_are_shipped(self) -> None:
        # The executable acceptance suites prove these entry points on temporary repos.
        for relative in (
            "gate-and-fix/scripts/repo_gate.py",
            "gate-and-fix/scripts/tests/generic_gate_test.py",
            "rebase-origin-main/scripts/rebase_base.py",
            "rebase-origin-main/scripts/tests/rebase_base_test.py",
        ):
            with self.subTest(path=relative):
                self.assertTrue((SKILLS / relative).is_file())

    def test_portable_skills_ship_policy_and_submodule_mechanics(self) -> None:
        for relative in (
            "rebase-origin-main/scripts/repository_context.py",
            "rebase-origin-main/scripts/submodule_state.py",
            "rebase-origin-main/scripts/tests/submodule_state_test.py",
            "review-pr/scripts/tests/generic_review_test.py",
        ):
            with self.subTest(path=relative):
                self.assertTrue((SKILLS / relative).is_file())

    def test_canonical_workflow_inventory_and_entrypoints(self) -> None:
        actual = {
            entry.name
            for entry in SKILLS.iterdir()
            if entry.is_dir() and (entry / "SKILL.md").is_file()
        }
        self.assertEqual(actual, CANONICAL_SKILLS | {"workflow-create"})
        self.assertTrue(PRODUCT_ONLY_SKILLS.isdisjoint(actual))
        for name in CANONICAL_SKILLS:
            with self.subTest(skill=name):
                entrypoint = SKILLS / name / "SKILL.md"
                self.assertLessEqual(len(entrypoint.read_text().splitlines()), 250)

    def test_agent_registries_do_not_shadow_user_installs(self) -> None:
        for registry in (REPO / ".agents" / "skills", REPO / ".claude" / "skills"):
            self.assertFalse(registry.exists(), f"repo skills shadow user copies: {registry}")

    def test_shared_workflow_sources_exist(self) -> None:
        required = (
            "docs/guides/review-format.md",
            "docs/guides/plan-review-guide.md",
            "docs/guides/code-review-guide.md",
            "docs/guides/review-response-guide.md",
            "docs/guides/architecture-principles.md",
            "docs/templates/plan-template.md",
            "cli_extensions/review_artifact.py",
            "cli_extensions/review_artifact_parser.py",
            "cli_extensions/review_artifact_text.py",
            "cli_extensions/review_artifact_types.py",
            "cli_extensions/review_round_common.py",
        )
        for relative in required:
            with self.subTest(path=relative):
                self.assertTrue((REPO / relative).is_file())

    def test_pr_host_and_installed_pr_paths_are_portable(self) -> None:
        import os
        import shutil
        import subprocess
        import tempfile

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            home, repo, installed = root / "home", root / "project", root / "skills"
            home.mkdir()
            repo.mkdir()
            env = {**os.environ, "HOME": str(home), "GIT_CONFIG_NOSYSTEM": "1"}
            subprocess.run(["git", "init", "-q", str(repo)], env=env, check=True)
            shutil.copytree(SKILLS / "pr", installed / "pr")
            shutil.copytree(SKILLS / "_bus", installed / "_bus")
            plan = repo / "plan.md"
            plan.write_text("## 目标\n\nPortable goal\n\n## 非目标\n\nNone\n")
            script = installed / "pr/scripts/pr_goal_context.py"
            done = subprocess.run(
                [sys.executable, str(script), "--branch", "feat/portable", "--plan", str(plan),
                 "--output", str(repo / "context.md")],
                cwd=repo, env=env, text=True, capture_output=True,
            )
            self.assertEqual(done.returncode, 0, done.stderr)
            lock = repo / "temp/review-pr/feat-portable/.locked-goal"
            self.assertTrue(lock.is_file(), done.stdout)
            self.assertEqual(lock.read_text(), "Portable goal\n")
            self.assertFalse((installed / "temp").exists())
            host = subprocess.run(
                [sys.executable, "-I", str(installed / "_bus/cli_extensions/pr_host.py"), "--help"],
                cwd=repo, env=env, text=True, capture_output=True,
            )
            self.assertEqual(host.returncode, 0, host.stderr)
            for name in ("pr", "merge-pr", "address-review-comments"):
                text = (SKILLS / name / "SKILL.md").read_text()
                self.assertNotRegex(text, r"python3 (?:skills/|cli_extensions/)")

    def test_review_verdicts_are_the_canonical_closed_sets(self) -> None:
        verdicts = _review_types_module().LEGAL_VERDICTS
        self.assertEqual(
            verdicts["plan"],
            {"可执行（Ready）", "需要完善（Needs Refinement）", "废弃（Abandon）"},
        )
        self.assertEqual(
            verdicts["pr"],
            {
                "Ready",
                "Needs Refinement",
                "Abandon",
                "Abandon (dimension 6: multi-purpose, split with split-pr)",
            },
        )
        # Task review left with execute-plan; only plan and PR reviews remain.
        self.assertEqual(set(verdicts), {"plan", "pr"})
        markdown = "\n".join(
            path.read_text(encoding="utf-8")
            for root in (SKILLS, REPO / "docs", REPO / "orchestration", REPO / "workflows")
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
            "docs/guides/review-format.md",
            "docs/templates/plan-template.md",
            "workflows/bug-triage-table-template.md",
            "workflows/handover-template.md",
            "workflows/investigation-report-template.md",
            "workflows/template.md",
            "skills/delete-dead-code/references/dead-code-findings-format.md",
            "skills/delete-dead-code/references/duplicate-findings-format.md",
            "skills/e2e-test/references/e2e-run-report-format.md",
            "skills/gate-and-fix/references/gate-round-format.md",
            "skills/pr/references/pr-template.md",
            "skills/_bus/docs/pr-template.md",
            "skills/_bus/docs/pull-request-template.md",
            "skills/split-pr/references/split-plan-format.md",
            "skills/worktree-new/references/repo-profile-format.md",
        }
        actual = {
            path.relative_to(REPO).as_posix()
            for root in (REPO / "docs", REPO / "skills", REPO / "workflows")
            for path in root.rglob("*.md")
            if path.name.endswith(("template.md", "format.md"))
        }
        self.assertEqual(actual, expected)

        combined = "\n".join((REPO / path).read_text() for path in sorted(expected))
        for stale in (
            "App Server",
            "cmake/Coverage.cmake",
            "docs/architecture/process-environment.md",
            "shell/packages/",
            "LibTV PR 模板",
        ):
            with self.subTest(stale=stale):
                self.assertNotIn(stale, combined)


if __name__ == "__main__":
    unittest.main()
