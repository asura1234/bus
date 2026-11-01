"""Isolated regressions for user-scope skill safety and relocation."""
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOL = [sys.executable, str(ROOT / "tools" / "install_skills.py")]


class SkillInstallSafetyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="skill-safety-", dir=os.environ.get("TMPDIR"))
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "home"
        self.home.mkdir()
        self.protected = self.root / "protected"
        self.protected.mkdir()
        self.env = {"PATH": os.environ["PATH"], "HOME": str(self.home),
                    "USERPROFILE": str(self.home), "TMPDIR": str(self.root),
                    "TEST_BUS_PROTECTED_HOME": str(self.protected), "PYTHONNOUSERSITE": "1"}

    def tool(self, *args, env=None):
        return subprocess.run([*TOOL, *args], cwd=self.root, env=env or self.env,
                              capture_output=True, text=True, check=False, timeout=60)

    def install(self):
        result = self.tool("install", "--yes")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        status = self.tool("status")
        self.assertEqual(status.returncode, 0, status.stderr)
        return json.loads(status.stdout)

    def test_parallel_installs_leave_a_complete_owned_manifest(self):
        children = [subprocess.Popen([*TOOL, "install", "--yes"],
                    cwd=self.root, env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                    text=True) for _ in range(6)]
        for child in children:
            out, err = child.communicate(timeout=60)
            self.assertEqual(child.returncode, 0, err + out)
            self.assertIn("claude: installed ", out)
        report = json.loads(self.tool("status").stdout)
        manifest = json.loads((self.home / ".config/bus-skills/manifest.json").read_text())
        self.assertEqual(len(manifest["installed"]), len(report["skills"]))
        self.assertEqual(list(self.home.glob("*/skills/.bus-*")), [])

    def test_partial_manifest_never_adopts_an_unmanaged_tree(self):
        self.install()
        state = self.home / ".config/bus-skills/manifest.json"
        manifest = json.loads(state.read_text())
        del manifest["installed"]["codex/review-pr"]
        state.write_text(json.dumps(manifest))
        skill = self.home / ".agents/skills/review-pr/SKILL.md"
        skill.write_text("user-owned replacement")
        report = self.install()
        self.assertEqual(skill.read_text(), "user-owned replacement")
        self.assertTrue(any(item["name"] == "review-pr" and item["provider"] == "codex"
                            and item["state"] == "conflict" for item in report["skills"]))

    def test_symlinked_provider_and_registry_and_skill_are_preserved(self):
        outside = self.root / "outside"
        outside.mkdir()
        (self.home / ".agents").symlink_to(outside, target_is_directory=True)
        (self.home / ".claude/skills").mkdir(parents=True)
        (self.home / ".claude/skills/pr").symlink_to(outside, target_is_directory=True)
        self.install()
        self.assertEqual(list(outside.iterdir()), [])

    def test_installed_review_support_imports_from_an_unrelated_repo(self):
        self.install()
        repo = self.root / "other-repo"
        repo.mkdir()
        subprocess.run(["git", "init", "--initial-branch=feature", "--template="], cwd=repo,
                       env=self.env, capture_output=True, check=True)
        script = self.home / ".agents/skills/address-review-comments/scripts/prepare_review_input.py"
        result = subprocess.run([sys.executable, str(script), "--help"], cwd=repo, env=self.env,
                                capture_output=True, text=True, check=False, timeout=15)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_installed_goal_writer_locks_the_callers_repo(self):
        self.install()
        repo = self.root / "other-repo"
        repo.mkdir()
        subprocess.run(["git", "init", "--initial-branch=feature", "--template="], cwd=repo,
                       env=self.env, capture_output=True, check=True)
        for name, content in [("goal.txt", "test user install"), ("non-goals.txt", "none"), ("origin.txt", "test user install")]:
            (repo / name).write_text(content)
        script = self.home / ".agents/skills/pr/scripts/pr_goal_context.py"
        result = subprocess.run([sys.executable, str(script), "--branch", "feature", "--output",
                    "temp/goal-context.md", "--goal-file", "goal.txt", "--non-goal-file", "non-goals.txt",
                    "--origin-file", "origin.txt"], cwd=repo, env=self.env, capture_output=True, text=True, check=False, timeout=15)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        misplaced = self.home / ".agents/temp/review-pr/feature/.locked-goal"
        self.assertTrue((repo / "temp/review-pr/feature/.locked-goal").is_file(),
                        result.stdout + f"misplaced lock exists: {misplaced.is_file()} at {misplaced}")
        self.assertFalse((self.home / ".agents/temp").exists())

    def test_symlinked_config_dir_is_not_written_through(self):
        outside = self.protected / "config"
        outside.mkdir()
        sentinel = outside / "sentinel"
        sentinel.write_text("keep")
        (self.home / ".config").symlink_to(outside, target_is_directory=True)
        before = sentinel.read_bytes()
        result = self.tool("install", "--yes")
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertEqual(sentinel.read_bytes(), before)
        self.assertEqual([path.name for path in outside.iterdir()], ["sentinel"])
        self.assertFalse((self.home / ".local/share/bus").exists())

    def test_preserved_user_pr_does_not_break_installed_review_pr(self):
        user = self.home / ".agents/skills/pr"
        user.mkdir(parents=True)
        (user / "SKILL.md").write_text("user-authored PR workflow")
        report = self.install()
        self.assertEqual((user / "SKILL.md").read_text(), "user-authored PR workflow")
        self.assertTrue(any(item["provider"] == "codex" and item["name"] == "review-pr"
                            and item["state"] == "installed" for item in report["skills"]))
        repo = self.root / "other-repo"
        repo.mkdir()
        subprocess.run(["git", "init", "--initial-branch=feature", "--template="], cwd=repo,
                       env=self.env, capture_output=True, check=True)
        (repo / ".git/info").mkdir()
        (repo / ".git/info/exclude").write_text("temp/\n")
        for content in ("base", "change"):
            (repo / "file.txt").write_text(content)
            subprocess.run(["git", "add", "file.txt"], cwd=repo, env=self.env, check=True)
            subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@example.com",
                            "commit", "-m", content], cwd=repo, env=self.env, check=True, capture_output=True)
        for name, content in [("goal.txt", "preserve user skills"), ("non-goals.txt", "none"),
                              ("origin.txt", "test skill independence")]:
            (self.root / name).write_text(content)
        support = self.home / ".agents/skills/_bus/cli_extensions/pr_goal_context.py"
        goal = subprocess.run([sys.executable, str(support), "--branch", "feature", "--output",
                    "temp/context.md", "--goal-file", str(self.root / "goal.txt"),
                    "--non-goal-file", str(self.root / "non-goals.txt"),
                    "--origin-file", str(self.root / "origin.txt")], cwd=repo, env=self.env,
                    capture_output=True, text=True, check=False, timeout=15)
        self.assertEqual(goal.returncode, 0, goal.stdout + goal.stderr)
        script = self.home / ".agents/skills/review-pr/scripts/review_round.py"
        result = subprocess.run([sys.executable, str(script), "--base", "HEAD~1", "--reviewer", "preserved"],
                                cwd=repo, env=self.env, capture_output=True, text=True, check=False, timeout=15)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("ROUND=1", result.stdout)
        shared = self.home / ".agents/skills/_bus"
        formatter = subprocess.run([sys.executable, str(shared / "cli_extensions/pr_format_check.py"), "--help"],
                                  cwd=repo, env=self.env, capture_output=True, text=True, check=False)
        self.assertEqual(formatter.returncode, 0, formatter.stdout + formatter.stderr)
        self.assertTrue((shared / "docs/pr-template.md").is_file())
        self.assertTrue((shared / "docs/pull-request-template.md").is_file())
        self.assertEqual((user / "SKILL.md").read_text(), "user-authored PR workflow")

if __name__ == "__main__":
    unittest.main(verbosity=2)
