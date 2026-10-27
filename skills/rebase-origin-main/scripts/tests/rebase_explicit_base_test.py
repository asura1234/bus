"""A caller-selected base must work in a selective clone of a feature branch."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

RESOLVER = Path(__file__).resolve().parents[1] / "rebase_base.py"

class RebaseExplicitBaseTest(unittest.TestCase):
    def test_fresh_rebase_uses_explicit_base_when_default_branch_is_not_fetched(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            origin, seed, repo = root / "origin", root / "seed", root / "selective"
            def git(path, *args):
                return subprocess.check_output(["git", "-C", str(path), *args], text=True).strip()
            subprocess.run(["git", "init", "--bare", "-q", "-b", "trunk", str(origin)], check=True)
            subprocess.run(["git", "init", "-q", "-b", "trunk", str(seed)], check=True)
            git(seed, "config", "user.name", "Base Test")
            git(seed, "config", "user.email", "base@example.test")
            git(seed, "commit", "--allow-empty", "-qm", "base")
            base = git(seed, "rev-parse", "HEAD")
            git(seed, "checkout", "-qb", "feature")
            git(seed, "commit", "--allow-empty", "-qm", "feature work")
            git(seed, "remote", "add", "origin", str(origin))
            git(seed, "push", "-q", "origin", "trunk:trunk", "feature:feature")
            subprocess.run(["git", "clone", "-q", "--single-branch", "--branch", "feature", str(origin), str(repo)], check=True)
            git(repo, "fetch", "-q", "origin")
            self.assertEqual(git(repo, "config", "--get", "remote.origin.fetch"), "+refs/heads/feature:refs/remotes/origin/feature")
            ordinary = subprocess.run([sys.executable, str(RESOLVER), "--repo", str(repo), "--base", base,
                                       "--check-branch", "feature"], text=True, capture_output=True)
            self.assertEqual(ordinary.returncode, 0, ordinary.stderr)
            self.assertEqual(ordinary.stdout.strip(), base)
            fresh = subprocess.run([sys.executable, str(RESOLVER), "--repo", str(repo), "--base", base,
                                    "--refresh-default", "--check-branch", "feature"], text=True, capture_output=True)
            self.assertEqual(fresh.returncode, 0, fresh.stderr)
            self.assertEqual(fresh.stdout.strip(), base)
            protected = subprocess.run([sys.executable, str(RESOLVER), "--repo", str(repo), "--base", base,
                                        "--refresh-default", "--check-branch", "trunk"], text=True, capture_output=True)
            self.assertEqual(protected.returncode, 2)
            self.assertIn("protected branch", protected.stderr)
            self.assertEqual(protected.stdout, "")

if __name__ == "__main__":
    unittest.main()
