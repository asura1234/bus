import json
import shutil
import sys
import unittest
from unittest.mock import patch

from repo_profile_test import Fixture, load


class ProvisionTests(Fixture):
    def setUp(self):
        super().setUp()
        self.runner = load("worktree_new")
        self.slot = self.root / "build-slot"
        self.disk = patch.object(
            shutil,
            "disk_usage",
            return_value=shutil._ntuple_diskusage(100 * 2**30, 40 * 2**30, 60 * 2**30),
        )
        self.disk.start()
        self.addCleanup(self.disk.stop)
        self.write(
            "build.py", 'from pathlib import Path\nPath("built").write_text("yes")\n'
        )
        self.write(
            "smoke.py",
            'from pathlib import Path\nassert Path("built").read_text()=="yes"\nPath("ran").write_text("yes")\n',
        )
        self.override()
        self.commit()

    def override(self, smoke=None, extra=""):
        steps = [
            {"cwd": ".", "argv": [sys.executable, "build.py"]},
            {"cwd": ".", "argv": [sys.executable, smoke or "smoke.py"]},
        ]
        # JSON strings/arrays are also valid for these TOML scalar values.
        value = (
            "[worktree]\nsmoke=["
            + ",".join('{cwd=".",argv=' + json.dumps(c["argv"]) + "}" for c in steps)
            + "]\n"
        )
        self.write(".bus/repo.toml", value + extra)

    def provision(self, **kwargs):
        return self.runner.provision(self.repo, build_slot=self.slot, **kwargs)

    def test_ready_only_after_build_and_headless_smoke_with_temp_home(self):
        result = self.provision()
        self.assertEqual(result["status"], "ready")
        self.assertEqual(result["head"], self.git("rev-parse", "HEAD"))
        self.assertTrue((self.repo / "ran").exists())
        self.assertFalse(self.slot.exists())
        self.assertFalse((self.home / ".claude").exists())
        self.assertFalse((self.home / ".codex").exists())
        self.assertEqual(
            json.loads((self.repo / "temp/.bus/worktree-ready.json").read_text()),
            result,
        )

    def test_failed_smoke_replaces_old_ready_and_releases_slot(self):
        self.provision()
        self.write("smoke.py", "raise SystemExit(17)\n")
        self.commit()
        with self.assertRaisesRegex(ValueError, "17"):
            self.provision()
        evidence = json.loads((self.repo / "temp/.bus/worktree-ready.json").read_text())
        self.assertEqual(evidence["status"], "failed")
        self.assertFalse(self.slot.exists())

    def test_low_disk_refuses_before_build_and_rechecks_each_phase(self):
        for free in (24.99, 25):
            with self.subTest(free=free):
                with patch.object(
                    shutil,
                    "disk_usage",
                    return_value=shutil._ntuple_diskusage(
                        100 * 2**30, 0, int(free * 2**30)
                    ),
                ):
                    if free < 25:
                        with self.assertRaisesRegex(ValueError, "25 GiB"):
                            self.provision()
                        self.assertFalse((self.repo / "built").exists())
                    else:
                        self.assertEqual(self.provision()["status"], "ready")
        (self.repo / "built").unlink()
        usages = [
            shutil._ntuple_diskusage(100 * 2**30, 0, n * 2**30) for n in (60, 60, 24)
        ]
        with patch.object(shutil, "disk_usage", side_effect=usages):
            with self.assertRaisesRegex(ValueError, "25 GiB"):
                self.provision()
        self.assertFalse(self.slot.exists())

    def test_node_pin_mismatch_blocks_before_build_and_tool_bin_resolves_it(self):
        self.write(".node-version", "24.19.0\n")
        self.commit()
        wrong = self.root / "wrong"
        correct = self.root / "correct"
        for directory, version in ((wrong, "v25.6.1"), (correct, "v24.19.0")):
            directory.mkdir()
            executable = directory / "node"
            executable.write_text('#!/bin/sh\nprintf "' + version + '\\n"\n')
            executable.chmod(0o755)
        with self.assertRaisesRegex(ValueError, "24.19.0"):
            self.provision(tool_bins=[wrong])
        self.assertFalse((self.repo / "built").exists())
        self.assertEqual(self.provision(tool_bins=[correct])["status"], "ready")

    def test_existing_shared_node_dependencies_reused_without_install_or_modification(
        self,
    ):
        self.write("package.json", '{"scripts":{"build":"unused","test":"unused"}}')
        self.write("package-lock.json", "{}")
        self.override(
            extra='shared_dependencies=["node_modules"]\nprepare=[{cwd=".",argv=["npm","ci"]}]\n'
        )
        deps = self.repo / "node_modules"
        deps.mkdir()
        (deps / "owned").write_text("main dependencies")
        self.commit()
        wt = self.root / "linked"
        self.git("worktree", "add", "-qb", "feature", str(wt))
        result = self.runner.provision(wt, build_slot=self.slot)
        self.assertEqual(result["status"], "ready")
        self.assertTrue((wt / "node_modules").is_symlink())
        self.assertEqual((deps / "owned").read_text(), "main dependencies")
        self.assertFalse(any(c["argv"] == ["npm", "ci"] for c in result["commands"]))

    def test_unsafe_existing_deps_symlink_and_lock_mismatch_block(self):
        deps = self.root / "external-deps"
        deps.mkdir()
        (self.repo / "node_modules").symlink_to(deps, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "shared_dependencies"):
            self.provision()
        self.assertTrue((self.repo / "node_modules").is_symlink())

    def test_manifest_or_source_mutation_cannot_report_ready(self):
        self.write(
            "smoke.py",
            'from pathlib import Path\nPath("build.py").write_text("changed")\n',
        )
        self.commit()
        with self.assertRaisesRegex(ValueError, "changed"):
            self.provision()
        self.assertEqual(
            json.loads((self.repo / "temp/.bus/worktree-ready.json").read_text())[
                "status"
            ],
            "failed",
        )

    def test_shared_runtime_directory_cannot_be_written_by_smoke(self):
        deps = self.root / "runtime-deps"
        deps.mkdir()
        (self.repo / "build").mkdir()
        (self.repo / "build/deps").symlink_to(deps, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "build/deps"):
            self.provision()
        self.assertFalse((self.repo / "built").exists())

    def test_lock_mismatch_refuses_shared_dependencies_and_preserves_source(self):
        self.write("package.json", '{"scripts":{"build":"unused","test":"unused"}}')
        self.write("package-lock.json", "{}")
        self.override(extra='shared_dependencies=["node_modules"]\nprepare=[]\n')
        (self.repo / "node_modules").mkdir()
        self.commit()
        wt = self.root / "linked"
        self.git("worktree", "add", "-qb", "feature", str(wt))
        (wt / "package-lock.json").write_text('{"changed":true}')
        self.git("add", "package-lock.json", cwd=wt)
        self.git("commit", "-qm", "different lock", cwd=wt)
        with self.assertRaisesRegex(ValueError, "different"):
            self.runner.provision(wt, build_slot=self.slot)
        self.assertFalse((wt / "node_modules").exists())
        self.assertTrue((self.repo / "node_modules").is_dir())

    def test_occupied_build_slot_is_preserved_and_does_not_run(self):
        self.slot.mkdir()
        owner = self.slot / "owner"
        owner.write_text("other worker")
        with self.assertRaises(ValueError):
            self.provision()
        self.assertEqual(owner.read_text(), "other worker")
        self.assertFalse((self.repo / "built").exists())

    def test_create_uses_remote_main_with_no_master_upstream_and_rejects_other_directory(
        self,
    ):
        origin = self.root / "origin.git"
        self.git("clone", "--bare", str(self.repo), str(origin))
        self.git("remote", "add", "origin", str(origin))
        destination = self.root / "new"
        self.runner.create(self.repo, "feat/generic", destination)
        self.assertEqual(
            self.git("branch", "--show-current", cwd=destination), "feat/generic"
        )
        self.assertEqual(
            self.git("for-each-ref", "--format=%(upstream)", "refs/heads/feat/generic"),
            "",
        )
        self.assertEqual(
            self.git("rev-parse", "HEAD", cwd=destination),
            self.git("rev-parse", "main"),
        )
        other = self.root / "unregistered"
        other.mkdir()
        with self.assertRaisesRegex(ValueError, "registered"):
            self.runner.create(self.repo, "other", other)


class HazardTests(Fixture):
    def setUp(self):
        super().setUp()
        self.profile = load("repo_profile")

    def test_h1_shared_runtime_receipts_warn_and_cannot_be_opted_into_sharing(self):
        shared = self.root / "deps"
        shared.mkdir()
        (self.repo / "build").mkdir()
        (self.repo / "build/deps").symlink_to(shared, target_is_directory=True)
        self.write("ci/deps/media-runtime-lock.json", '{"releaseId":"older"}')
        result = self.profile.scan(self.repo)
        hazard = next(h for h in result["hazards"] if h["id"] == "shared-deps")
        self.assertIn("releaseId", hazard["reason"])
        self.write(".bus/repo.toml", '[worktree]\nshared_dependencies=["build/deps"]\n')
        with self.assertRaisesRegex(ValueError, "runtime"):
            self.profile.scan(self.repo)
        self.assertTrue((self.repo / "build/deps").is_symlink())

    def test_h2_fresh_gitlink_gap_warns_without_generated_file_repairs(self):
        self.write(
            ".gitmodules",
            '[submodule "lib"]\npath=external/lib\nurl=https://example.test/lib.git\n',
        )
        self.write("external/lib-placeholder", "no ignored next-env.d.ts")
        result = self.profile.scan(self.repo)
        hazard = next(h for h in result["hazards"] if h["id"] == "fresh-gitlink")
        self.assertIn("git ls-tree origin/main external/lib", hazard["action"])
        self.assertIn("pnpm exec tsc", hazard["action"])
        self.assertFalse((self.repo / "external/lib/next-env.d.ts").exists())

    def test_h3_uninitialized_submodule_records_unknown_pins_instead_of_assuming_none(
        self,
    ):
        self.write(
            ".gitmodules",
            '[submodule "lldi"]\npath=external/lldi\nurl=https://example.test/lldi.git\n',
        )
        result = self.profile.scan(self.repo)
        self.assertFalse(result["submodules"][0]["initialized"])
        self.assertIn("submodule-pins", {h["id"] for h in result["hazards"]})
        self.assertTrue(result["recipe"]["unresolved"])

    def test_h4_node_abi_hazard_tracks_pin_change_in_cache(self):
        self.write(".node-version", "24.19.0\n")
        first = self.profile.scan(self.repo)
        self.assertIn("node-abi", {h["id"] for h in first["hazards"]})
        self.write(".node-version", "24.20.0\n")
        second = self.profile.scan(self.repo)
        self.assertNotEqual(first["manifest_hash"], second["manifest_hash"])
        self.assertEqual(second["toolchains"][0]["pin"], "24.20.0")

    def test_h5_no_app_required_and_disk_failure_warning_is_recorded(self):
        self.write("run", "#!/bin/sh\n# app session requires a GUI\n")
        result = self.profile.scan(self.repo)
        self.assertIn("disk-floor", {h["id"] for h in result["hazards"]})
        self.assertEqual(result["recipe"]["smoke"], [])
        self.assertTrue(result["recipe"]["unresolved"])

    def test_h6_rerere_shared_config_warns_without_mutating_it(self):
        self.git("config", "rerere.enabled", "true")
        self.rust()
        result = self.profile.scan(self.repo)
        hazard = next(h for h in result["hazards"] if h["id"] == "rerere")
        self.assertIn("git show --name-only", hazard["action"])
        self.assertEqual(self.git("config", "--bool", "rerere.enabled"), "true")

    def test_h7_checkout_planning_budget_is_recorded(self):
        self.rust()
        result = self.profile.scan(self.repo)
        self.assertGreaterEqual(result["recipe"]["disk_estimate_gib"], 15)
        hazard = next(h for h in result["hazards"] if h["id"] == "worktree-budget")
        self.assertIn("Existing worktrees: 1", hazard["action"])

    def test_h8_nested_rust_target_in_budget_and_cleanup_inventory(self):
        self.rust()
        self.write("native/rust/Cargo.toml", "[workspace]\nmembers=[]\n")
        result = self.profile.scan(self.repo)
        self.assertIn("native/rust/target", result["recipe"]["target_paths"])
        self.assertGreaterEqual(result["recipe"]["disk_estimate_gib"], 19)
        hazard = next(h for h in result["hazards"] if h["id"] == "nested-target")
        self.assertIn("native/rust/target", hazard["action"])

    def test_forge_policy_is_typed_default_false_and_override_true(self):
        self.rust()
        self.assertFalse(
            self.profile.scan(self.repo)["policy"]["require_exact_head_gate"]
        )
        self.write(
            ".bus/repo.toml", 'gate_command="just ci"\nrequire_exact_head_gate=true\n'
        )
        policy = self.profile.scan(self.repo)["policy"]
        self.assertTrue(policy["require_exact_head_gate"])
        self.assertEqual(policy["gate_command"], "just ci")
        self.write(".bus/repo.toml", 'require_exact_head_gate="true"\n')
        with self.assertRaisesRegex(ValueError, "boolean"):
            self.profile.scan(self.repo)


if __name__ == "__main__":
    unittest.main()
