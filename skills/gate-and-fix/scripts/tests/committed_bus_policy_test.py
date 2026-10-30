"""The committed installable skill set must include Bus's profile and scanner."""
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[4]


class CommittedBusPolicyTest(unittest.TestCase):
    def test_committed_bus_bundle_selects_existing_adapter_without_override(self):
        archive = subprocess.check_output([
            "git", "-C", str(SOURCE), "archive", "HEAD", "skills/gate-and-fix",
            "skills/worktree-new", ".bus/repo.toml", ".gitignore",
        ])
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            with tarfile.open(fileobj=io.BytesIO(archive)) as tree:
                for entry in tree:
                    if not entry.isfile():
                        continue
                    target = repo / entry.name
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(tree.extractfile(entry).read())
            subprocess.run(["git", "init", "-q", "-b", "feature", str(repo)], check=True)
            subprocess.run(["git", "-C", str(repo), "add", "--force", ".bus/repo.toml", ".gitignore", "skills"], check=True)
            subprocess.run(["git", "-C", str(repo), "-c", "user.name=Test", "-c", "user.email=test@example.test", "commit", "-qm", "committed snapshot"], check=True)
            script = (
                "import json,sys;from pathlib import Path;"
                "sys.path.insert(0,sys.argv[1]);from repo_gate import gate_policy;"
                "print(json.dumps(gate_policy(Path.cwd(),None)))"
            )
            result = subprocess.run([
                sys.executable, "-c", script, str(repo / "skills/gate-and-fix/scripts")
            ], cwd=repo, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout), ["just ci", True])


if __name__ == "__main__":
    unittest.main()
