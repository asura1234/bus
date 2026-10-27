"""Exercise base discovery with a real origin whose default branch is trunk."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

RESOLVER = Path(__file__).resolve().parents[1] / "rebase_base.py"


class RebaseBaseTest(unittest.TestCase):
    def test_default_and_explicit_base_drive_real_rebase(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            origin, repo = root / "origin", root / "repo"
            subprocess.run(["git", "init", "--bare", "-q", "-b", "trunk", str(origin)], check=True)
            subprocess.run(["git", "clone", "-q", str(origin), str(repo)], check=True)
            def git(*args):
                return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()
            git("config", "user.name", "Rebase Test")
            git("config", "user.email", "rebase@example.test")
            (repo / "base").write_text("base\n")
            git("add", "base")
            git("commit", "-qm", "initial")
            git("push", "-q", "origin", "trunk:trunk")
            git("remote", "set-head", "origin", "-a")
            first = git("rev-parse", "HEAD")
            git("checkout", "-qb", "feature")
            (repo / "feature").write_text("feature\n")
            git("add", "feature")
            git("commit", "-qm", "feature")
            git("checkout", "-q", "trunk")
            (repo / "base").write_text("new base\n")
            git("commit", "-qam", "new base")
            git("push", "-q", "origin", "trunk:trunk")
            expected = git("rev-parse", "origin/trunk")
            git("checkout", "-q", "feature")
            result = subprocess.run([sys.executable, str(RESOLVER)], cwd=repo, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.strip(), expected)
            git("rebase", result.stdout.strip())
            self.assertEqual(git("rev-parse", "HEAD^"), expected)
            explicit = subprocess.run([sys.executable, str(RESOLVER), "--base", first], cwd=repo, text=True, capture_output=True)
            self.assertEqual(explicit.returncode, 0, explicit.stderr)
            self.assertEqual(explicit.stdout.strip(), first)
            git("symbolic-ref", "--delete", "refs/remotes/origin/HEAD")
            missing = subprocess.run([sys.executable, str(RESOLVER)], cwd=repo, text=True, capture_output=True)
            self.assertNotEqual(missing.returncode, 0)
            self.assertEqual(missing.stdout, "")
            self.assertIn("--base", missing.stderr)
            invalid = subprocess.run([sys.executable, str(RESOLVER), "--base", "missing"], cwd=repo, text=True, capture_output=True)
            self.assertNotEqual(invalid.returncode, 0)
            self.assertEqual(invalid.stdout, "")


if __name__ == "__main__":
    unittest.main()
