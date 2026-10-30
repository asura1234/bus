import json
import subprocess
import sys
from repo_profile_test import Fixture, SCRIPTS


class GitPathTests(Fixture):
    def test_carriage_return_in_repository_path_is_preserved(self):
        renamed = self.root / "main\rworktree"
        self.repo.rename(renamed)
        self.repo = renamed
        self.rust()
        self.commit()
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPTS / "repo_profile.py"),
                "scan",
                "--no-cache",
                "--repo",
                str(self.repo),
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout)["main_worktree"], str(self.repo.resolve())
        )
