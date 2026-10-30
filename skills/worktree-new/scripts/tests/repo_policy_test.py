import json
import subprocess
import sys
from repo_profile_test import Fixture, SCRIPTS


class PolicyTests(Fixture):
    def test_policy_command_ignores_inventory_and_other_worktree_configuration(self):
        self.write(".gitignore", "")
        self.write("package.json", "{}")
        self.write("package-lock.json", "{}")
        self.write("yarn.lock", "# another lock")
        self.write(
            ".bus/repo.toml",
            'gate_command="just ci"\nrequire_exact_head_gate=true\n[worktree]\nshared_dependencies=[["not-a-path"]]\n',
        )
        self.commit()
        exclude = self.repo / ".git/info/exclude"
        before = exclude.read_bytes()
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPTS / "repo_profile.py"),
                "policy",
                "--repo",
                str(self.repo),
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout),
            {
                "policy": {
                    "gate_command": "just ci",
                    "require_exact_head_gate": True,
                    "commit_style": None,
                    "submodule_update": "recursive",
                }
            },
        )
        self.assertFalse((self.repo / "temp").exists())
        self.assertEqual(exclude.read_bytes(), before)
        self.assertEqual(self.git("status", "--porcelain"), "")
