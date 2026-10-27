import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))


def load(name):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Fixture(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.home = self.root / "home"
        self.home.mkdir()
        self.env = patch.dict(
            os.environ, {"HOME": str(self.home), "GIT_CONFIG_GLOBAL": os.devnull}
        )
        self.env.start()
        self.addCleanup(self.env.stop)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.git("init", "-b", "main")
        self.git("config", "user.email", "fixture@example.test")
        self.git("config", "user.name", "Fixture")
        self.write(".gitignore", "temp/\nnode_modules\nbuild/\n")

    def git(self, *args, cwd=None):
        return subprocess.check_output(
            ["git", "-C", str(cwd or self.repo), *args], text=True
        ).strip()

    def write(self, path, text):
        dest = self.repo / path
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(text)
        return dest

    def commit(self):
        self.git("add", ".")
        self.git("commit", "-qm", "fixture")

    def rust(self):
        self.write(
            "Cargo.toml", '[package]\nname="fixture"\nversion="0.1.0"\nedition="2021"\n'
        )
        self.write("Cargo.lock", "version = 4\n")
        self.write("rust-toolchain.toml", '[toolchain]\nchannel="1.96.1"\n')


class ProfileTests(Fixture):
    def setUp(self):
        super().setUp()
        self.profile = load("repo_profile")

    def test_no_cache_cli_keeps_repo_without_temp_ignore_clean(self):
        self.rust()
        self.write(".gitignore", "")
        self.commit()
        argv = [
            sys.executable,
            str(SCRIPTS / "repo_profile.py"),
            "scan",
            "--no-cache",
            "--repo",
            str(self.repo),
        ]
        result = subprocess.run(argv, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        fresh = json.loads(result.stdout)
        self.assertFalse((self.repo / "temp").exists())
        self.assertEqual(self.git("status", "--porcelain"), "")
        cached = self.profile.scan(self.repo)
        self.assertEqual(fresh, cached)
        cache = self.repo / "temp/.bus/repo-profile.json"
        cache.write_text("corrupt cache; do not read or replace")
        before = cache.stat().st_mtime_ns
        status = self.git("status", "--porcelain")
        result = subprocess.run(argv, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), fresh)
        self.assertEqual(cache.read_text(), "corrupt cache; do not read or replace")
        self.assertEqual(cache.stat().st_mtime_ns, before)
        self.assertEqual(self.git("status", "--porcelain"), status)

    def test_unchanged_manifest_cache_skips_inference_but_refreshes_local_hazards(self):
        self.rust()
        first = self.profile.scan(self.repo)
        with patch.object(
            self.profile, "inventory", side_effect=AssertionError("rescanned")
        ):
            self.assertEqual(self.profile.scan(self.repo), first)
            self.git("config", "rerere.enabled", "true")
            refreshed = self.profile.scan(self.repo)
        self.assertIn("rerere", {h["id"] for h in refreshed["hazards"]})
        self.write("rust-toolchain.toml", '[toolchain]\nchannel="1.95.0"\n')
        changed = self.profile.scan(self.repo)
        self.assertNotEqual(changed["manifest_hash"], first["manifest_hash"])
        self.assertEqual(changed["toolchains"][0]["pin"], "1.95.0")

    def test_malformed_cache_is_recomputed_instead_of_used(self):
        self.rust()
        first = self.profile.scan(self.repo)
        cache = self.repo / "temp/.bus/repo-profile.json"
        missing_policy = dict(first)
        del missing_policy["policy"]
        for malformed in (
            dict(first, recipe={"smoke": [{"argv": []}]}),
            missing_policy,
            dict(first, package_managers=[{}]),
        ):
            with self.subTest(cache=malformed):
                cache.write_text(json.dumps(malformed))
                self.assertEqual(self.profile.scan(self.repo), first)

    def test_rust_locked_recipe_and_cache_shared_by_linked_worktrees(self):
        self.rust()
        self.commit()
        wt = self.root / "linked"
        self.git("worktree", "add", "-qb", "feature", str(wt))
        first = self.profile.scan(wt)
        cache = self.repo / "temp/.bus/repo-profile.json"
        self.assertEqual(json.loads(cache.read_text()), first)
        stamp = cache.stat().st_mtime_ns
        self.assertEqual(self.profile.scan(wt), first)
        self.assertEqual(cache.stat().st_mtime_ns, stamp)
        self.assertEqual(
            first["recipe"]["prepare"][0]["argv"], ["cargo", "fetch", "--locked"]
        )
        self.assertEqual(
            first["recipe"]["smoke"][-1]["argv"], ["cargo", "test", "--locked"]
        )
        (wt / "Cargo.lock").write_text("version = 3\n")
        changed = self.profile.scan(wt)
        self.assertNotEqual(changed["manifest_hash"], first["manifest_hash"])
        self.assertEqual(changed["toolchains"][0]["pin"], "1.96.1")

    def test_node_workspace_and_lock_manager(self):
        self.write(
            "package.json",
            json.dumps(
                {
                    "packageManager": "pnpm@10.0.0",
                    "workspaces": ["packages/*"],
                    "scripts": {
                        "build": "tsc",
                        "test:ci": "vitest run",
                        "start": "electron .",
                    },
                }
            ),
        )
        self.write("pnpm-lock.yaml", "lockfileVersion: 9\n")
        self.write("packages/ui/package.json", '{"name":"ui"}')
        result = self.profile.scan(self.repo)
        self.assertEqual(result["package_managers"][0]["manager"], "pnpm")
        self.assertEqual(result["package_managers"][0]["workspaces"], ["packages/*"])
        self.assertEqual(
            result["recipe"]["prepare"][0]["argv"],
            ["pnpm", "install", "--frozen-lockfile"],
        )
        self.assertEqual(
            [c["argv"] for c in result["recipe"]["smoke"]],
            [["pnpm", "run", "build"], ["pnpm", "run", "test:ci"]],
        )

    def test_react_native_inventory_does_not_claim_gui_is_smoke(self):
        self.write(
            "package.json",
            '{"scripts":{"ios":"react-native run-ios","android":"react-native run-android"}}',
        )
        self.write("package-lock.json", "{}")
        self.write("ios/App.xcodeproj/project.pbxproj", "// fixture")
        self.write("ios/Podfile", "platform :ios, '15.0'")
        self.write("android/build.gradle", "plugins {}")
        self.write("android/gradlew", "#!/bin/sh\n")
        result = self.profile.scan(self.repo)
        kinds = {t["kind"] for t in result["toolchains"]}
        self.assertTrue({"xcode", "gradle"}.issubset(kinds))
        self.assertTrue(result["recipe"]["unresolved"])
        self.assertFalse(any("run-ios" in str(c) for c in result["recipe"]["smoke"]))

    def test_submodule_pin_and_gitlink_invalidate_hash(self):
        sub = self.root / "source"
        sub.mkdir()
        self.git("init", "-b", "main", cwd=sub)
        self.git("config", "user.email", "fixture@example.test", cwd=sub)
        self.git("config", "user.name", "Fixture", cwd=sub)
        (sub / "Cargo.toml").write_text('[package]\nname="native"\nversion="0.1.0"\n')
        (sub / "rust-toolchain.toml").write_text('[toolchain]\nchannel="1.85.0"\n')
        self.git("add", ".", cwd=sub)
        self.git("commit", "-qm", "native", cwd=sub)
        self.git(
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            str(sub),
            "native/core",
        )
        self.commit()
        first = self.profile.scan(self.repo)
        self.assertIn("1.85.0", [t["pin"] for t in first["toolchains"]])
        self.assertEqual(first["recipe"]["target_paths"], [])
        self.assertEqual(first["submodules"][0]["path"], "native/core")
        (self.repo / "native/core/rust-toolchain.toml").write_text(
            '[toolchain]\nchannel="1.86.0"\n'
        )
        self.assertNotEqual(
            first["manifest_hash"], self.profile.scan(self.repo)["manifest_hash"]
        )

    def test_cmake_make_just_run_inventory_and_override(self):
        self.write("CMakeLists.txt", "project(fixture)\nenable_testing()\n")
        self.write("Makefile", "build:\n\ttrue\n")
        self.write("justfile", "test:\n    true\n")
        self.write("run", "#!/bin/sh\n")
        self.write(
            ".bus/repo.toml",
            'gate_command="just ci"\ncommit_style="conventional"\n'
            '[worktree]\ndisk_estimate_gib=8\nsmoke=[{cwd=".",argv=["./run","test"]}]\n',
        )
        result = self.profile.scan(self.repo)
        self.assertEqual(result["policy"]["gate_command"], "just ci")
        self.assertEqual(result["recipe"]["disk_estimate_gib"], 8)
        self.assertEqual(result["recipe"]["smoke"][0]["argv"], ["./run", "test"])
        self.assertEqual(
            {e["kind"] for e in result["entry_points"]},
            {"cmake", "make", "just", "run"},
        )

    def test_pnpm_workspace_file_is_inferred_and_detached_scan_works(self):
        self.write("package.json", '{"packageManager":"pnpm@10.0.0"}')
        self.write("pnpm-lock.yaml", "lockfileVersion: 9\n")
        self.write("pnpm-workspace.yaml", "packages:\n  - 'apps/*'\n  - packages/*\n")
        self.commit()
        self.git("checkout", "--detach", "-q")
        result = self.profile.scan(self.repo)
        self.assertEqual(
            result["package_managers"][0]["workspaces"], ["apps/*", "packages/*"]
        )

    def test_symlinked_override_is_refused_even_when_ignored(self):
        outside = self.root / "override.toml"
        outside.write_text('gate_command="true"')
        self.write(".gitignore", ".bus/\ntemp/\n")
        (self.repo / ".bus").mkdir()
        (self.repo / ".bus/repo.toml").symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "symlink"):
            self.profile.scan(self.repo)

    def test_conflicting_lockfiles_unknown_overrides_and_escape_fail_closed(self):
        self.write("package.json", "{}")
        self.write("package-lock.json", "{}")
        self.write("yarn.lock", "# fixture")
        with self.assertRaisesRegex(ValueError, "lockfile"):
            self.profile.scan(self.repo)
        (self.repo / "yarn.lock").unlink()
        self.write(".bus/repo.toml", "[worktree]\nsmkoe=[]\n")
        with self.assertRaisesRegex(ValueError, "unknown"):
            self.profile.scan(self.repo)
        self.write(
            ".bus/repo.toml", '[worktree]\nsmoke=[{cwd="../outside",argv=["true"]}]\n'
        )
        with self.assertRaisesRegex(ValueError, "relative"):
            self.profile.scan(self.repo)


if __name__ == "__main__":
    unittest.main()
