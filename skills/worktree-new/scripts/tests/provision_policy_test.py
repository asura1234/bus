import json
import shutil
import subprocess
import sys
from unittest.mock import patch

from repo_profile_test import Fixture, load, SCRIPTS


class Base(Fixture):
    def setUp(self):
        super().setUp()
        self.runner = load("worktree_new")
        self.profile = load("repo_profile")
        self.slot = self.root / "build-slot"
        disk = patch.object(
            shutil,
            "disk_usage",
            return_value=shutil._ntuple_diskusage(100 * 2**30, 40 * 2**30, 60 * 2**30),
        )
        disk.start()
        self.addCleanup(disk.stop)
        self.write(
            "build.py", 'from pathlib import Path\nPath("built").write_text("yes")\n'
        )
        self.write(
            "smoke.py", 'from pathlib import Path\nPath("ran").write_text("yes")\n'
        )

    def smoke_toml(self, extra=""):
        steps = [[sys.executable, "build.py"], [sys.executable, "smoke.py"]]
        return (
            "[worktree]\nsmoke=["
            + ",".join('{cwd=".",argv=' + json.dumps(a) + "}" for a in steps)
            + "]\n"
            + extra
        )

    def tool(self, name, body):
        bindir = self.root / "bin"
        bindir.mkdir(exist_ok=True)
        exe = bindir / name
        exe.write_text("#!/bin/sh\n" + body)
        exe.chmod(0o755)
        return bindir


class SharedDependencyWriteThrough(Base):
    def test_workspace_member_sharing_is_refused_before_root_install(self):
        # Root pnpm workspace; only the member's node_modules is opted into sharing.
        self.write("package.json", '{"scripts":{"build":"x","test":"x"}}')
        self.write("pnpm-lock.yaml", "lockfileVersion: '9.0'\n")
        self.write("pnpm-workspace.yaml", "packages:\n  - packages/*\n")
        self.write("packages/a/package.json", '{"name":"a"}')
        self.write(
            ".bus/repo.toml",
            self.smoke_toml('shared_dependencies=["packages/a/node_modules"]\n'),
        )
        (self.repo / "packages/a/node_modules").mkdir()
        self.commit()
        wt = self.root / "linked"
        self.git("worktree", "add", "-qb", "feature", str(wt))
        bindir = self.tool(
            "pnpm",
            "mkdir -p packages/a/node_modules && touch packages/a/node_modules/feature-marker\n",
        )
        with self.assertRaisesRegex(ValueError, "lock|workspace"):
            self.runner.provision(wt, build_slot=self.slot, tool_bins=[bindir])
        self.assertFalse(
            (self.repo / "packages/a/node_modules/feature-marker").exists(),
            "feature checkout install wrote through shared symlink into main's node_modules",
        )

    def test_install_alias_is_skipped_for_shared_node_modules(self):
        self.write("package.json", '{"scripts":{"build":"x","test":"x"}}')
        self.write("pnpm-lock.yaml", "lockfileVersion: '9.0'\n")
        self.write(
            ".bus/repo.toml",
            self.smoke_toml(
                'shared_dependencies=["node_modules"]\nprepare=[{cwd=".",argv=["pnpm","i","--frozen-lockfile"]}]\n'
            ),
        )
        (self.repo / "node_modules").mkdir()
        self.commit()
        wt = self.root / "linked"
        self.git("worktree", "add", "-qb", "feature", str(wt))
        bindir = self.tool("pnpm", "touch node_modules/feature-marker\n")
        self.runner.provision(wt, build_slot=self.slot, tool_bins=[bindir])
        self.assertFalse(
            (self.repo / "node_modules/feature-marker").exists(),
            "`pnpm i` was executed against the shared main node_modules",
        )


class NodePins(Base):
    def test_unused_nested_pin_does_not_block_root_commands(self):
        self.write(".node-version", "24.19.0\n")
        self.write("examples/legacy/.nvmrc", "18.20.0\n")
        self.write(".bus/repo.toml", self.smoke_toml())
        self.commit()
        bindir = self.tool("node", 'printf "v24.19.0\\n"\n')
        result = self.runner.provision(
            self.repo, build_slot=self.slot, tool_bins=[bindir]
        )
        self.assertEqual(result["status"], "ready")


class OverrideValidation(Base):
    def test_non_string_submodule_update_is_a_validation_error(self):
        self.write(".bus/repo.toml", '[submodules]\nupdate=["recursive"]\n')
        self.commit()
        with self.assertRaises(ValueError):
            self.profile.scan(self.repo, use_cache=False)

    def test_non_string_shared_dependency_entry_is_a_validation_error(self):
        self.write(
            ".bus/repo.toml", '[worktree]\nshared_dependencies=[["node_modules"]]\n'
        )
        self.commit()
        with self.assertRaises(ValueError):
            self.profile.scan(self.repo, use_cache=False)

    def test_cli_reports_not_ready_instead_of_traceback(self):
        self.write(".bus/repo.toml", '[submodules]\nupdate=["recursive"]\n')
        self.commit()
        out = subprocess.run(
            [
                sys.executable,
                "-B",
                str(SCRIPTS / "worktree_new.py"),
                "provision",
                "--repo",
                str(self.repo),
                "--build-slot",
                str(self.slot),
            ],
            capture_output=True,
            text=True,
        )
        self.assertIn("NOT READY", out.stderr)
        self.assertNotIn("Traceback", out.stderr)


class Detection(Base):
    def test_watch_all_and_simulator_tests_are_not_headless_smoke(self):
        for script in (
            "jest --watchAll",
            "detox test -c ios.sim.debug",
            "jest --watchAll=false",
        ):
            with self.subTest(script=script):
                self.write(
                    "package.json",
                    json.dumps({"scripts": {"build": "tsc", "test": script}}),
                )
                self.write("package-lock.json", "{}")
                result = self.profile.scan(self.repo, use_cache=False)
                assertion = (
                    self.assertIn if script.endswith("=false") else self.assertNotIn
                )
                assertion(
                    ["npm", "run", "test"],
                    [c["argv"] for c in result["recipe"]["smoke"]],
                )


class NumericNodePins(Base):
    def test_major_pin_accepts_matching_release(self):
        self.write(".nvmrc", "20\n")
        self.write(".bus/repo.toml", self.smoke_toml())
        self.commit()
        bindir = self.tool("node", 'printf "v20.11.1\\n"\n')
        self.assertEqual(
            self.runner.provision(self.repo, build_slot=self.slot, tool_bins=[bindir])[
                "status"
            ],
            "ready",
        )


class CommandNodePins(Base):
    def test_each_command_uses_its_nearest_pin_and_supplied_binary(self):
        self.write(".node-version", "20\n")
        self.write("legacy/.nvmrc", "24.19\n")
        bins = [self.root / "node20", self.root / "node24"]
        for directory, version in zip(bins, ("20.11.1", "24.19.0")):
            directory.mkdir()
            node = directory / "node"
            node.write_text('#!/bin/sh\nprintf "v' + version + '\\n"\n')
            node.chmod(0o755)
        profile = self.profile.scan(self.repo, use_cache=False)
        env = self.runner.child_env(self.repo, bins)
        self.runner.verify_node(self.repo, profile, env, "legacy", bins)
        self.assertEqual(
            subprocess.check_output(["node", "--version"], env=env, text=True).strip(),
            "v24.19.0",
        )
        self.runner.verify_node(self.repo, profile, env, ".", bins)
        self.assertEqual(
            subprocess.check_output(["node", "--version"], env=env, text=True).strip(),
            "v20.11.1",
        )
