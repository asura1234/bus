import json
import os
import shutil
import unittest
from unittest.mock import patch

from repo_profile_test import Fixture, load


class WorktreeSafetyTests(Fixture):
    def setUp(self):
        super().setUp()
        self.runner = load("worktree_new")

    def test_single_branch_clone_can_create_from_live_default(self):
        self.write("source.txt", "default branch source\n")
        self.commit()
        default_head = self.git("rev-parse", "HEAD")
        self.git("branch", "topic")
        origin = self.root / "origin.git"
        self.git("clone", "--bare", str(self.repo), str(origin))
        checkout = self.root / "topic-checkout"
        self.git(
            "clone",
            "--single-branch",
            "--branch",
            "topic",
            str(origin),
            str(checkout),
        )
        self.assertEqual(
            self.git("config", "--get", "remote.origin.fetch", cwd=checkout),
            "+refs/heads/topic:refs/remotes/origin/topic",
        )
        destination = self.root / "new-feature"
        self.runner.create(checkout, "feature/new", destination)
        self.assertEqual(self.git("rev-parse", "HEAD", cwd=destination), default_head)
        self.assertEqual(
            self.git(
                "for-each-ref",
                "--format=%(upstream)",
                "refs/heads/feature/new",
                cwd=checkout,
            ),
            "",
        )

    @unittest.skipUnless(
        shutil.which("node") and shutil.which("npm"), "Node/npm required"
    )
    def test_shared_workspace_source_is_refused_before_smoke(self):
        self.write(
            "package.json",
            json.dumps(
                {
                    "name": "workspace-fixture",
                    "private": True,
                    "workspaces": ["packages/*"],
                    "scripts": {"build": "node build.cjs", "test": "node smoke.cjs"},
                }
            ),
        )
        self.write("package-lock.json", '{"lockfileVersion":3,"packages":{}}')
        self.write(
            "packages/ui/package.json", '{"name":"@fixture/ui","main":"index.cjs"}'
        )
        self.write("packages/ui/index.cjs", 'module.exports = "main";\n')
        self.write(
            "build.cjs",
            'require("node:assert").strictEqual(typeof require("@fixture/ui"), "string");\n',
        )
        self.write(
            "smoke.cjs",
            'const value = require("@fixture/ui");\n'
            'require("node:assert").ok(["main", "feature"].includes(value));\n'
            'require("node:fs").writeFileSync("tested-value", value);\n',
        )
        self.write(
            ".bus/repo.toml", '[worktree]\nshared_dependencies=["node_modules"]\n'
        )
        package_link = self.repo / "node_modules/@fixture/ui"
        package_link.parent.mkdir(parents=True)
        package_link.symlink_to("../../packages/ui", target_is_directory=True)
        self.commit()
        linked = self.root / "linked"
        self.git("worktree", "add", "-qb", "feature", str(linked))
        (linked / "packages/ui/index.cjs").write_text('module.exports = "feature";\n')
        self.git("add", "packages/ui/index.cjs", cwd=linked)
        self.git("commit", "-qm", "change feature source", cwd=linked)
        with patch.object(
            self.runner.shutil,
            "disk_usage",
            return_value=shutil._ntuple_diskusage(100 * 2**30, 0, 60 * 2**30),
        ):
            with self.assertRaisesRegex(ValueError, "outside|workspace"):
                self.runner.provision(linked, build_slot=self.root / "slot")
        self.assertFalse((linked / "tested-value").exists())
        self.assertEqual(
            (self.repo / "packages/ui/index.cjs").read_text(),
            'module.exports = "main";\n',
        )
        self.assertEqual(
            (linked / "packages/ui/index.cjs").read_text(),
            'module.exports = "feature";\n',
        )

    def test_child_cargo_jobs_respects_lower_caller_limit(self):
        with patch.dict(os.environ, {"CARGO_BUILD_JOBS": "1"}):
            with patch.object(self.runner.os, "cpu_count", return_value=16):
                env = self.runner.child_env(self.repo, [])
        self.assertLessEqual(int(env["CARGO_BUILD_JOBS"]), 1)


if __name__ == "__main__":
    unittest.main()
