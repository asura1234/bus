import json
import shutil
import sys
from unittest.mock import patch

from repo_profile_test import Fixture, load


class PrivatePathRuntimeTests(Fixture):
    def setUp(self):
        super().setUp()
        self.runner = load("worktree_new")
        self.shared = self.root / "shared-runtime"
        self.shared.mkdir()
        self.slot = self.root / "slot"
        disk = patch.object(
            self.runner.shutil,
            "disk_usage",
            return_value=shutil._ntuple_diskusage(100 * 2**30, 0, 60 * 2**30),
        )
        disk.start()
        self.addCleanup(disk.stop)
        self.link = [
            sys.executable,
            "-c",
            f"from pathlib import Path; Path('state').symlink_to({str(self.shared)!r}, target_is_directory=True)",
        ]
        self.receipt = [
            sys.executable,
            "-c",
            "from pathlib import Path; Path('state/receipt').write_text('feature')",
        ]
        self.build = [
            sys.executable,
            "-c",
            "from pathlib import Path; Path('built').touch()",
        ]

    def configure(self, prepare, smoke):
        def commands(steps):
            return (
                "["
                + ",".join('{cwd=".",argv=' + json.dumps(argv) + "}" for argv in steps)
                + "]"
            )

        self.write(
            ".bus/repo.toml",
            '[worktree]\nprivate_paths=["state"]\n'
            + "prepare="
            + commands(prepare)
            + "\n"
            + "smoke="
            + commands(smoke)
            + "\n",
        )
        self.commit()
        linked = self.root / "linked"
        self.git("worktree", "add", "-qb", "feature", str(linked))
        return linked

    def assert_refused(self, linked):
        with self.assertRaisesRegex(ValueError, "private runtime state"):
            self.runner.provision(linked, build_slot=self.slot)
        self.assertFalse((self.shared / "receipt").exists())
        self.assertFalse(self.slot.exists())
        evidence = json.loads((linked / "temp/.bus/worktree-ready.json").read_text())
        self.assertEqual(evidence["status"], "failed")
        self.assertIn("state", evidence["error"])

    def test_private_path_linked_outside_by_prepare_is_refused_before_smoke_writes(
        self,
    ):
        linked = self.configure([self.link], [self.build, self.receipt])
        self.assert_refused(linked)
        self.assertFalse((linked / "built").exists())

    def test_prepare_cannot_expose_private_state_to_the_next_prepare_command(self):
        linked = self.configure([self.link, self.receipt], [self.build, self.receipt])
        self.assert_refused(linked)

    def test_smoke_cannot_expose_private_state_to_the_next_smoke_command(self):
        linked = self.configure([], [self.build, self.link, self.receipt])
        self.assert_refused(linked)

    def test_final_smoke_link_cannot_report_ready(self):
        linked = self.configure([], [self.build, self.link])
        self.assert_refused(linked)

    def test_prepare_can_create_local_private_state_and_smoke_uses_it(self):
        create = [
            sys.executable,
            "-c",
            "from pathlib import Path; Path('state').mkdir()",
        ]
        linked = self.configure([create], [self.build, self.receipt])
        result = self.runner.provision(linked, build_slot=self.slot)
        self.assertEqual(result["status"], "ready")
        self.assertEqual((linked / "state/receipt").read_text(), "feature")
        self.assertFalse((self.shared / "receipt").exists())
        self.assertFalse(self.slot.exists())
