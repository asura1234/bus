"""Protect worktree-local submodule branches before removing their parent."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "submodule_state.py"


class ChildBranchCleanupProbe(unittest.TestCase):
    def test_local_child_branch_is_reported_before_force_removal(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = {**os.environ, "GIT_ALLOW_PROTOCOL": "file",
                   "GIT_AUTHOR_NAME": "Test", "GIT_AUTHOR_EMAIL": "test@example.test",
                   "GIT_COMMITTER_NAME": "Test", "GIT_COMMITTER_EMAIL": "test@example.test"}

            def git(repo, *args):
                return subprocess.check_output(["git", "-C", str(repo), *args], env=env, text=True).strip()

            source, main, worktree = root / "source", root / "main", root / "feature"
            for repo in (source, main):
                repo.mkdir()
                git(repo, "init", "-q", "-b", "trunk")
                (repo / "file").write_text("base\n")
                git(repo, "add", "file")
                git(repo, "commit", "-qm", "base")
            git(main, "worktree", "add", "-q", "-b", "feature", str(worktree))
            git(worktree, "submodule", "add", "-q", str(source), "dependency")
            git(worktree, "add", ".gitmodules", "dependency")
            git(worktree, "commit", "-qm", "record child")
            git(main, "update-ref", "refs/remotes/origin/feature", git(worktree, "rev-parse", "HEAD"))
            child = worktree / "dependency"
            pinned = git(child, "rev-parse", "HEAD")
            git(child, "switch", "-qc", "local-only")
            (child / "file").write_text("unpublished branch work\n")
            git(child, "commit", "-qam", "unpublished branch work")
            unique = git(child, "rev-parse", "HEAD")
            git(child, "switch", "--detach", "-q", pinned)
            self.assertEqual(git(worktree, "status", "--short"), "")
            self.assertEqual(git(worktree, "log", "--oneline", "HEAD", "--not", "--remotes"), "")
            self.assertEqual(git(child, "log", "--oneline", "HEAD", "--not", "--remotes"), "")
            run = subprocess.run([sys.executable, str(SCRIPT), "check", "--repo", str(worktree)],
                                 env=env, text=True, capture_output=True)
            self.assertEqual(run.returncode, 1, "local child branch not reported: " + run.stdout + run.stderr)
            self.assertIn(unique, run.stdout)


if __name__ == "__main__":
    unittest.main()
