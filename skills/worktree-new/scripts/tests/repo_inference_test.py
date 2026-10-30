import json
import shutil
import subprocess
import sys

from repo_profile_test import Fixture, load, SCRIPTS


class RealWorldShapeTests(Fixture):
    def setUp(self):
        super().setUp()
        self.profile = load("repo_profile")

    def test_workspace_member_crates_share_the_workspace_target(self):
        self.write(
            "native/rust/Cargo.toml", '[workspace]\nmembers=["a","b"]\nresolver="2"\n'
        )
        self.write("native/rust/Cargo.lock", "version = 4\n")
        for crate in ("a", "b"):
            self.write(
                f"native/rust/{crate}/Cargo.toml",
                f'[package]\nname="{crate}"\nversion="0.1.0"\nedition="2021"\n',
            )
        self.commit()
        recipe = self.profile.scan(self.repo, use_cache=False)["recipe"]
        self.assertEqual(recipe["target_paths"], ["native/rust/target"])
        self.assertEqual(recipe["disk_estimate_gib"], 19)

    def test_run_wrapper_with_nested_node_product_is_not_ready_from_cargo_alone(self):
        self.write("run", '#!/bin/sh\nexec node scripts/run.js "$@"\n')
        self.write(
            "apps/desktop/package.json",
            '{"scripts":{"build":"tsc","test":"vitest run"}}',
        )
        self.write("apps/desktop/pnpm-lock.yaml", "lockfileVersion: '9.0'\n")
        self.write(
            "native/rust/Cargo.toml",
            '[package]\nname="n"\nversion="0.1.0"\nedition="2021"\n',
        )
        self.write("native/rust/Cargo.lock", "version = 4\n")
        self.commit()
        recipe = self.profile.scan(self.repo, use_cache=False)["recipe"]
        self.assertNotEqual(recipe["unresolved"], [], recipe["smoke"])

    def test_source_upgrade_invalidates_cached_recipe(self):
        self.rust()
        self.commit()
        tools = self.root / "scanner"
        tools.mkdir()
        for path in SCRIPTS.glob("*.py"):
            shutil.copy2(path, tools / path.name)
        argv = [
            sys.executable,
            str(tools / "repo_profile.py"),
            "scan",
            "--repo",
            str(self.repo),
        ]
        first = json.loads(subprocess.check_output(argv, text=True))
        changed = False
        for path in tools.glob("*.py"):
            text = path.read_text()
            if '["cargo", "build", "--locked", *workspace_scope]' in text:
                path.write_text(
                    text.replace(
                        '["cargo", "build", "--locked", *workspace_scope]',
                        '["cargo", "check", "--locked", *workspace_scope]',
                    )
                )
                changed = True
        self.assertTrue(changed, "fixture must actually upgrade the scanner source")
        second = json.loads(subprocess.check_output(argv, text=True))
        self.assertEqual(
            second["recipe"]["smoke"][0]["argv"], ["cargo", "check", "--locked"]
        )
        self.assertNotEqual(first["scanner_hash"], second["scanner_hash"])

    def test_cached_scan_keeps_main_checkout_clean_without_temp_ignore(self):
        self.write(".gitignore", "node_modules\n")
        self.rust()
        self.commit()
        self.profile.scan(self.repo)
        self.assertEqual(self.git("status", "--porcelain"), "")

    def test_dependency_crates_do_not_own_extra_targets(self):
        self.rust()
        with (self.repo / "Cargo.toml").open("a") as out:
            out.write('[dependencies]\nhelper={path="third_party/helper"}\n')
        self.write(
            "third_party/helper/Cargo.toml",
            '[package]\nname="helper"\nversion="0.1.0"\n',
        )
        result = self.profile.scan(self.repo, use_cache=False)["recipe"]
        self.assertEqual(result["target_paths"], ["target"])
        self.assertEqual(result["disk_estimate_gib"], 15)

    def test_independent_external_directory_is_not_assumed_to_be_vendor(self):
        self.write(
            "external/product/Cargo.toml",
            '[package]\nname="product"\nversion="0.1.0"\n',
        )
        self.write("external/product/Cargo.lock", "version=4\n")
        result = self.profile.scan(self.repo, use_cache=False)["recipe"]
        self.assertIn(
            {"cwd": "external/product", "argv": ["cargo", "build", "--locked"]},
            result["smoke"],
        )

    def test_configured_private_state_and_custom_lock_are_generic(self):
        target = self.root / "shared"
        target.mkdir()
        (self.repo / "cache").mkdir()
        (self.repo / "cache/sdk").symlink_to(target, target_is_directory=True)
        self.write("ci/sdk.pin", "v1")
        self.write(
            ".bus/repo.toml",
            '[worktree]\nprivate_paths=["cache/sdk/active"]\nextra_manifests=["ci/sdk.pin"]\n',
        )
        first = self.profile.scan(self.repo)
        state = next(h for h in first["hazards"] if h["id"] == "shared-deps")
        self.assertIn("cache/sdk", state["paths"])
        self.assertTrue(all("media-runtime" not in p for p in state["paths"]))
        self.assertEqual(first["recipe"]["private_paths"], ["cache/sdk/active"])
        self.write("ci/sdk.pin", "v2")
        self.assertNotEqual(
            first["manifest_hash"], self.profile.scan(self.repo)["manifest_hash"]
        )

    def test_artifact_exclusion_does_not_hide_user_temp_files(self):
        self.rust()
        self.write(".gitignore", "")
        self.commit()
        self.profile.scan(self.repo)
        self.write("temp/user-note.txt", "owned by user")
        self.assertIn(
            "?? temp/user-note.txt",
            self.git("status", "--porcelain", "--untracked-files=all"),
        )
        self.assertNotIn(
            "repo-profile.json",
            self.git("status", "--porcelain", "--untracked-files=all"),
        )
