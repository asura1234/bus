"""gate-and-fix's branch guard must protect origin's new default after a forge switch."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
RESOLVER = SCRIPTS / "rebase_base.py"
sys.path.insert(0, str(SCRIPTS))
from rebase_base import ensure_feature_branch  # noqa: E402


class GateGuardAfterDefaultSwitchTest(unittest.TestCase):
    def test_skill_sequence_protects_switched_default(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            origin, repo = root / "origin.git", root / "repo"
            subprocess.run(["git", "init", "--bare", "-q", "-b", "master", str(origin)], check=True)
            subprocess.run(["git", "init", "-q", "-b", "master", str(repo)], check=True)

            def git(*args):
                return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()

            git("config", "user.name", "T")
            git("config", "user.email", "t@example.test")
            git("commit", "--allow-empty", "-qm", "base")
            git("remote", "add", "origin", str(origin))
            git("push", "-q", "origin", "master")
            git("remote", "set-head", "origin", "--auto")
            # The forge switches its default branch to trunk.
            git("push", "-q", "origin", "master:trunk")
            subprocess.run(["git", "-C", str(origin), "symbolic-ref", "HEAD", "refs/heads/trunk"], check=True)
            git("-c", "fetch.followRemoteHEAD=never", "fetch", "-q", "origin")
            git("checkout", "-qb", "trunk", "origin/trunk")
            # gate-and-fix SKILL.md: base resolution with --refresh-default, then the branch guard.
            refreshed = subprocess.run([sys.executable, str(RESOLVER), "--repo", str(repo), "--refresh-default"],
                                       text=True, capture_output=True)
            self.assertEqual(refreshed.returncode, 0, refreshed.stderr)
            guard = subprocess.run([sys.executable, str(RESOLVER), "--repo", str(repo), "--base", "HEAD",
                                    "--check-branch", "trunk"], text=True, capture_output=True)
            self.assertEqual(guard.returncode, 2, "gate-and-fix guard let the new default branch trunk through")
            # gate_and_fix.py:281 runner guard.
            with self.assertRaises(ValueError):
                ensure_feature_branch(repo, "trunk")


if __name__ == "__main__":
    unittest.main()
