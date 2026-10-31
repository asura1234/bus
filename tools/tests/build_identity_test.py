from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class BuildIdentityTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.compiled = tempfile.TemporaryDirectory(prefix="bus-build-identity-bin-")
        cls.addClassCleanup(cls.compiled.cleanup)
        directory = Path(cls.compiled.name)
        cls.binary = directory / ("identity.exe" if os.name == "nt" else "identity")
        harness = directory / "harness.rs"
        harness.write_text(
            "mod production {\n"
            f"include!({json.dumps((ROOT / 'build_identity.rs').as_posix())});\n"
            "pub fn record() { record_build_identity(); }\n"
            "}\nfn main() { production::record(); }\n"
        )
        prefix = ["nice", "-n", "10"] if shutil.which("nice") else []
        subprocess.run(
            [*prefix, "rustc", "--edition=2021", "--cap-lints=allow", str(harness), "-o", str(cls.binary)],
            check=True,
            capture_output=True,
            text=True,
        )

    def git(self, root: Path, *args: str) -> str:
        return subprocess.check_output(
            [
                "git", "-c", f"core.hooksPath={self.hooks}",
                "-c", "commit.gpgsign=false", "-c", "core.fsmonitor=false",
                "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                *args,
            ],
            cwd=root,
            env=self.environment,
            text=True,
            stderr=subprocess.PIPE,
        ).strip()

    def setUp(self) -> None:
        self.fixture = tempfile.TemporaryDirectory(prefix="bus-build-identity-git-")
        self.addCleanup(self.fixture.cleanup)
        self.directory = Path(self.fixture.name)
        self.hooks = self.directory / "empty-hooks"
        self.hooks.mkdir()
        self.environment = {
            key: value for key, value in os.environ.items()
            if not key.startswith(("BUS_", "HERDR_", "GIT_"))
        }
        self.repo = self.directory / "repo"
        self.repo.mkdir()
        self.environment["BUS_CHECKOUT_ROOT"] = str(self.repo)
        self.git(self.repo, "init", "-q", "-b", "fixture", f"--template={self.hooks}")
        self.git(self.repo, "commit", "--allow-empty", "-q", "-m", "fixture")
        self.sha = self.git(self.repo, "rev-parse", "HEAD")

    def record(self, manifest: Path, extra: dict[str, str] | None = None, *, dev: bool = True) -> str:
        env = {**self.environment, **(extra or {}), "CARGO_MANIFEST_DIR": str(manifest), "BUS_CHECKOUT_ROOT": str(manifest), "PROFILE": "fixture"}
        if dev:
            env["CARGO_FEATURE_DEV_TOOLS"] = "1"
        else:
            env.pop("CARGO_FEATURE_DEV_TOOLS", None)
        return subprocess.check_output(
            [str(self.binary)],
            cwd=manifest,
            env=env,
            text=True,
        )

    def test_source_archive_never_claims_an_unrelated_parent_repository(self) -> None:
        source = self.repo / "source-archive"
        source.mkdir()
        output = self.record(source)
        self.assertIn("cargo:rustc-env=BUS_BUILD_SHA=unknown\n", output)
        self.assertIn("cargo:rustc-env=BUS_BUILD_SHORT_SHA=unknown\n", output)
        self.assertNotIn(self.sha, output)
        self.assertNotIn("cargo:rerun-if-changed=", output)

    def test_short_hash_tracks_clean_and_dirty_source(self) -> None:
        (self.repo / "src").mkdir()
        source = self.repo / "src/tracked.txt"
        source.write_text("clean\n")
        self.git(self.repo, "add", "src/tracked.txt")
        self.git(self.repo, "commit", "-q", "-m", "source")
        short = self.git(self.repo, "rev-parse", "--short", "HEAD")
        self.assertIn(f"cargo:rustc-env=BUS_BUILD_SHORT_SHA={short}\n", self.record(self.repo))
        source.write_text("dirty\n")
        self.assertIn(f"cargo:rustc-env=BUS_BUILD_SHORT_SHA={short}-dirty\n", self.record(self.repo))
        self.git(self.repo, "restore", "src/tracked.txt")
        (self.repo / "src/untracked.txt").write_text("new source\n")
        self.assertIn(f"cargo:rustc-env=BUS_BUILD_SHORT_SHA={short}-dirty\n", self.record(self.repo))

    def test_missing_git_uses_unknown_short_hash(self) -> None:
        output = self.record(self.repo, {"PATH": str(self.hooks)})
        self.assertIn("cargo:rustc-env=BUS_BUILD_SHORT_SHA=unknown\n", output)

    def test_non_dev_identity_includes_dirty_source(self) -> None:
        (self.repo / "src").mkdir()
        (self.repo / "src/new.rs").write_text("// untracked input\n")
        output = self.record(self.repo, dev=False)
        self.assertIn(f"cargo:rustc-env=BUS_BUILD_SHA={self.sha}\n", output)
        short = self.git(self.repo, "rev-parse", "--short", "HEAD")
        self.assertIn(f"cargo:rustc-env=BUS_BUILD_SHORT_SHA={short}-dirty\n", output)
        self.assertIn("cargo:rerun-if-changed=src\n", output)
        self.assertNotIn("cargo:rerun-if-changed=.git/index\n", output)

    def test_git_checkout_rejects_missing_or_wrong_checkout_marker(self) -> None:
        for marker in (None, str(self.directory)):
            env = {**self.environment, "CARGO_MANIFEST_DIR": str(self.repo), "PROFILE": "fixture"}
            if marker is None:
                env.pop("BUS_CHECKOUT_ROOT")
            else:
                env["BUS_CHECKOUT_ROOT"] = marker
            result = subprocess.run([str(self.binary)], cwd=self.repo, env=env,
                                    capture_output=True, text=True)
            with self.subTest(marker=marker):
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("BUS_CHECKOUT_ROOT", result.stderr)

    def test_checkout_does_not_use_ambient_git_directories(self) -> None:
        foreign = self.directory / "foreign"
        foreign.mkdir()
        self.git(foreign, "init", "-q", "-b", "fixture", f"--template={self.hooks}")
        self.git(foreign, "commit", "--allow-empty", "-q", "-m", "foreign")
        self.assertNotEqual(self.git(foreign, "rev-parse", "HEAD"), self.sha)
        for overrides in (
            {"GIT_DIR": str(foreign / ".git"), "GIT_WORK_TREE": str(foreign)},
            {"GIT_COMMON_DIR": str(foreign / ".git")},
        ):
            with self.subTest(variables=sorted(overrides)):
                output = self.record(self.repo, overrides)
                self.assertIn(f"cargo:rustc-env=BUS_BUILD_SHA={self.sha}\n", output)

    def test_checkout_and_git_file_worktree_keep_their_actual_identity(self) -> None:
        worktree = self.directory / "worktree"
        self.git(self.repo, "worktree", "add", "--detach", "-q", str(worktree), "HEAD")
        self.assertTrue((worktree / ".git").is_file())
        for source in (self.repo, worktree):
            with self.subTest(source=source.name):
                output = self.record(source)
                self.assertIn(f"cargo:rustc-env=BUS_BUILD_SHA={self.sha}\n", output)
                self.assertIn("cargo:rustc-env=BUS_BUILD_PROFILE=fixture\n", output)
                self.assertIn("cargo:rerun-if-changed=", output)

    def test_packed_branch_ref_emits_only_existing_rerun_paths(self) -> None:
        # `git gc` packs branch refs, deleting the loose ref file until the next commit.
        self.git(self.repo, "pack-refs", "--all", "--prune")
        output = self.record(self.repo)
        self.assertIn(f"cargo:rustc-env=BUS_BUILD_SHA={self.sha}\n", output)
        prefix = "cargo:rerun-if-changed="
        watched = [line[len(prefix):] for line in output.splitlines() if line.startswith(prefix)]
        self.assertTrue(watched)
        missing = [path for path in watched if not (self.repo / path).exists()]
        self.assertEqual(missing, [], output)

    def test_cargo_caches_packed_refs_and_notices_the_next_loose_commit(self) -> None:
        (self.repo / "Cargo.toml").write_text(
            '[package]\nname="freeze_identity_fixture"\nversion="0.0.0"\nedition="2021"\n'
            '[features]\ndev-tools=[]\n'
        )
        (self.repo / "src").mkdir()
        (self.repo / "src/main.rs").write_text(
            'fn main() { println!("{} {}", env!("BUS_BUILD_SHA"), '
            'option_env!("BUS_BUILD_SHORT_SHA").unwrap_or("missing")); }\n'
        )
        (self.repo / "build.rs").write_text(
            "mod production {\n"
            f"include!({json.dumps((ROOT / 'build_identity.rs').as_posix())});\n"
            "pub fn record() { record_build_identity(); }\n}\n"
            "fn main() { production::record(); }\n"
        )
        base = Path(os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target")))
        target = (base if base.is_absolute() else ROOT / base) / "build-identity-fixtures" / self.directory.name
        self.addCleanup(shutil.rmtree, target, True)
        env = {**self.environment, "CARGO_TARGET_DIR": str(target), "CARGO_BUILD_JOBS": "4"}
        for key in ("LLVM_PROFILE_FILE", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
            env.pop(key, None)
        prefix = ["nice", "-n", "10"] if shutil.which("nice") else []

        def cargo(*args: str) -> subprocess.CompletedProcess[str]:
            if args[0] == "build":
                args = (*args, "--features", "dev-tools")
            return subprocess.run([*prefix, "cargo", *args], cwd=self.repo, env=env,
                                  capture_output=True, text=True, check=True)

        cargo("generate-lockfile", "--offline")
        binary = target / "debug" / ("freeze_identity_fixture.exe" if os.name == "nt" else "freeze_identity_fixture")

        def build(expected_sha: str, cached: bool = False, dirty: bool = False) -> None:
            before = binary.stat().st_mtime_ns if binary.exists() else None
            result = cargo("build", "--locked", "--offline", "-j", "4", "--message-format", "json")
            short = self.git(self.repo, "rev-parse", "--short", expected_sha)
            expected = f"{expected_sha} {short}{'-dirty' if dirty else ''}"
            self.assertEqual(subprocess.check_output([str(binary)], text=True).strip(), expected)
            if cached:
                artifacts = [json.loads(line) for line in result.stdout.splitlines()]
                binary_artifact = next(item for item in artifacts
                                       if item.get("reason") == "compiler-artifact"
                                       and item["target"]["kind"] == ["bin"])
                self.assertTrue(binary_artifact["fresh"], result.stderr)
                self.assertEqual(binary.stat().st_mtime_ns, before, "unchanged build recompiled the crate")

        (self.repo / "src/tracked.txt").write_text("clean\n")
        self.git(self.repo, "add", "Cargo.toml", "Cargo.lock", "src/main.rs", "build.rs", "src/tracked.txt")
        self.git(self.repo, "commit", "-q", "-m", "fixture source")
        self.sha = self.git(self.repo, "rev-parse", "HEAD")
        self.git(self.repo, "pack-refs", "--all", "--prune")
        build(self.sha)
        build(self.sha, cached=True)
        self.git(self.repo, "commit", "--allow-empty", "-q", "-m", "next")
        next_sha = self.git(self.repo, "rev-parse", "HEAD")
        old_short = self.git(self.repo, "rev-parse", "--short", self.sha)
        self.assertEqual(subprocess.check_output([str(binary)], text=True).strip(),
                         f"{self.sha} {old_short}", "running binary keeps its embedded identity")
        build(next_sha)
        build(next_sha, cached=True)
        self.git(self.repo, "pack-refs", "--all", "--prune")
        build(next_sha)
        build(next_sha, cached=True)

        (self.repo / "src/tracked.txt").write_text("changed\n")
        build(next_sha, dirty=True)
        build(next_sha, cached=True, dirty=True)
        self.git(self.repo, "add", "src/tracked.txt")
        build(next_sha, dirty=True)
        self.git(self.repo, "restore", "--staged", "--worktree", "src/tracked.txt")
        build(next_sha)
        build(next_sha, cached=True)


    def cargo_fixture(self, *, dev: bool = True):
        (self.repo / "Cargo.toml").write_text(
            '[package]\nname="dirty_identity_fixture"\nversion="0.0.0"\nedition="2021"\n'
            '[features]\ndev-tools=[]\n'
        )
        (self.repo / "src").mkdir()
        (self.repo / "src/main.rs").write_text(
            'fn main() { println!("{}", option_env!("BUS_BUILD_SHORT_SHA").unwrap_or("non-dev")); }\n'
        )
        (self.repo / "build.rs").write_text(
            "mod production {\n"
            f"include!({json.dumps((ROOT / 'build_identity.rs').as_posix())});\n"
            "pub fn record() { record_build_identity(); }\n}\n"
            "fn main() { production::record(); }\n"
        )
        (self.repo / "notes.md").write_text("notes\n")
        (self.repo / "src/tracked.txt").write_text("clean\n")
        base = Path(os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target")))
        target = (base if base.is_absolute() else ROOT / base) / "build-identity-fixtures" / self.directory.name
        self.addCleanup(shutil.rmtree, target, True)
        env = {**self.environment, "CARGO_TARGET_DIR": str(target), "CARGO_BUILD_JOBS": "2"}
        for key in ("LLVM_PROFILE_FILE", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
            env.pop(key, None)

        def cargo(*args: str) -> subprocess.CompletedProcess[str]:
            if dev and args[0] == "build":
                args = (*args, "--features", "dev-tools")
            return subprocess.run(["cargo", *args], cwd=self.repo, env=env,
                                  capture_output=True, text=True, check=True)

        cargo("generate-lockfile", "--offline")
        self.git(self.repo, "add", "Cargo.toml", "Cargo.lock", "src", "build.rs", "notes.md")
        self.git(self.repo, "commit", "-q", "-m", "fixture source")
        binary = target / "debug" / "dirty_identity_fixture"

        def build() -> tuple[str, bool]:
            result = cargo("build", "--locked", "--offline", "--message-format", "json")
            artifacts = [json.loads(line) for line in result.stdout.splitlines()]
            fresh = next(item for item in artifacts
                         if item.get("reason") == "compiler-artifact"
                         and item["target"]["name"] == "dirty_identity_fixture")["fresh"]
            return subprocess.check_output([str(binary)], text=True).strip(), fresh

        return build

    def test_new_untracked_file_after_a_clean_build_marks_the_next_build_dirty(self) -> None:
        build = self.cargo_fixture()
        short = self.git(self.repo, "rev-parse", "--short", "HEAD")
        self.assertEqual(build()[0], short)
        (self.repo / "src/scratch.txt").write_text("new, not ignored\n")
        self.assertEqual(build()[0], f"{short}-dirty")

    def test_editing_a_tracked_non_source_file_in_a_dirty_tree_keeps_the_crate_fresh(self) -> None:
        build = self.cargo_fixture()
        short = self.git(self.repo, "rev-parse", "--short", "HEAD")
        (self.repo / "src/tracked.txt").write_text("dirty\n")
        self.assertEqual(build()[0], f"{short}-dirty")
        (self.repo / "notes.md").write_text("more notes\n")
        text, fresh = build()
        self.assertEqual(text, f"{short}-dirty")
        self.assertTrue(fresh, "an edit that leaves the embedded hash unchanged recompiled the crate")

    def test_non_source_edits_and_deletions_keep_a_clean_dev_build_cached(self) -> None:
        build = self.cargo_fixture()
        short = self.git(self.repo, "rev-parse", "--short", "HEAD")
        self.assertEqual(build()[0], short)
        (self.repo / "notes.md").write_text("unrelated edit\n")
        self.assertEqual(build(), (short, True))
        (self.repo / "notes.md").unlink()
        self.assertEqual(build(), (short, True))
        self.assertEqual(build(), (short, True))

    def test_non_dev_source_dirty_identity_and_unrelated_doc_changes(self) -> None:
        build = self.cargo_fixture(dev=False)
        short = self.git(self.repo, "rev-parse", "--short", "HEAD")
        self.assertEqual(build()[0], short)
        (self.repo / "src/new.rs").write_text("// unused untracked input\n")
        self.assertEqual(build()[0], f"{short}-dirty")
        (self.repo / "notes.md").unlink()
        self.assertEqual(build(), (f"{short}-dirty", True))

    def test_vendor_source_is_watched_but_generated_vendor_output_is_not(self) -> None:
        build = self.cargo_fixture()
        source = self.repo / "vendor/libghostty-vt/src"
        source.mkdir(parents=True)
        (source / "fixture.zig").write_text("// source\n")
        (self.repo / ".gitignore").write_text("vendor/libghostty-vt/.zig-cache/\n")
        self.git(self.repo, "add", "vendor/libghostty-vt/src", ".gitignore")
        self.git(self.repo, "commit", "-q", "-m", "vendored source")
        short = self.git(self.repo, "rev-parse", "--short", "HEAD")
        self.assertEqual(build()[0], short)
        generated = source.parent / ".zig-cache"
        generated.mkdir()
        (generated / "output").write_text("generated\n")
        self.assertEqual(build(), (short, True))
        (source / "new.zig").write_text("// new source\n")
        self.assertEqual(build()[0], f"{short}-dirty")
        self.assertEqual(build(), (f"{short}-dirty", True))


    def test_cargo_rechecks_dirty_identity_when_new_untracked_source_appears(self) -> None:
        (self.repo / "Cargo.toml").write_text(
            '[package]\nname="dirty_identity_fixture"\nversion="0.0.0"\nedition="2021"\n'
            '[features]\ndev-tools=[]\n'
        )
        (self.repo / "src").mkdir()
        (self.repo / "src/main.rs").write_text(
            'fn main() { println!("{}", env!("BUS_BUILD_SHORT_SHA")); }\n'
        )
        (self.repo / "build.rs").write_text(
            "mod production {\n"
            f"include!({json.dumps((ROOT / 'build_identity.rs').as_posix())});\n"
            "pub fn record() { record_build_identity(); }\n}\n"
            "fn main() { production::record(); }\n"
        )
        base = Path(os.environ.get("CARGO_TARGET_DIR", str(self.directory / "target")))
        target = base / "new-untracked-identity" / self.directory.name
        self.addCleanup(shutil.rmtree, target, True)
        env = {**self.environment, "CARGO_TARGET_DIR": str(target)}
        for key in ("LLVM_PROFILE_FILE", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
            env.pop(key, None)

        def cargo(*args: str) -> subprocess.CompletedProcess[str]:
            if args[0] == "build":
                args = (*args, "--features", "dev-tools")
            return subprocess.run(
                ["cargo", *args], cwd=self.repo, env=env,
                capture_output=True, text=True, check=True,
            )

        cargo("generate-lockfile", "--offline")
        self.git(self.repo, "add", "Cargo.toml", "Cargo.lock", "src/main.rs", "build.rs")
        self.git(self.repo, "commit", "-q", "-m", "fixture source")
        short = self.git(self.repo, "rev-parse", "--short", "HEAD")
        cargo("build", "--locked", "--offline", "-j", "4")
        binary = target / "debug" / (
            "dirty_identity_fixture.exe" if os.name == "nt" else "dirty_identity_fixture"
        )
        self.assertEqual(subprocess.check_output([str(binary)], text=True).strip(), short)
        (self.repo / "src/new-source.txt").write_text("new untracked source\n")
        cargo("build", "--locked", "--offline", "-j", "4")
        self.assertEqual(
            subprocess.check_output([str(binary)], text=True).strip(),
            f"{short}-dirty",
            "a new untracked source must invalidate the cached clean build identity",
        )


    def test_staging_an_unrelated_file_keeps_a_clean_dev_build_cached(self) -> None:
        build = self.cargo_fixture()
        short = self.git(self.repo, "rev-parse", "--short", "HEAD")
        self.assertEqual(build()[0], short)
        (self.repo / "notes.md").write_text("staged unrelated edit\n")
        self.git(self.repo, "add", "notes.md")
        text, fresh = build()
        self.assertEqual(text, short)
        self.assertTrue(fresh, "staging an unrelated file recompiled the crate")


class BuildIdentityCacheProbeTests(BuildIdentityTests):
    def short_hash_crate(self, root: Path):
        (root / "Cargo.toml").write_text(
            '[package]\nname="cache_identity_fixture"\nversion="0.0.0"\nedition="2021"\n'
            '[features]\ndev-tools=[]\n'
        )
        (root / "src").mkdir(exist_ok=True)
        (root / "src/main.rs").write_text(
            'fn main() { println!("{} {}", env!("BUS_BUILD_SHA"), env!("BUS_BUILD_SHORT_SHA")); }\n'
        )
        (root / "build.rs").write_text(
            "mod production {\n"
            f"include!({json.dumps((ROOT / 'build_identity.rs').as_posix())});\n"
            "pub fn record() { record_build_identity(); }\n}\n"
            "fn main() { production::record(); }\n"
        )
        (root / "notes.md").write_text("notes\n")
        (root / "src/retired.rs").write_text("// unused build input\n")
        base = Path(os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target")))
        target = (base if base.is_absolute() else ROOT / base) / "cache-identity-probe" / self.directory.name
        self.addCleanup(shutil.rmtree, target, True)
        env = {**self.environment, "CARGO_TARGET_DIR": str(target), "BUS_CHECKOUT_ROOT": str(root)}
        for key in ("LLVM_PROFILE_FILE", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
            env.pop(key, None)

        def cargo(*args: str) -> subprocess.CompletedProcess[str]:
            if args[0] == "build":
                args = (*args, "--features", "dev-tools")
            return subprocess.run(["cargo", *args], cwd=root, env=env,
                                  capture_output=True, text=True, check=True)

        cargo("generate-lockfile", "--offline")
        self.git(root, "add", "Cargo.toml", "Cargo.lock", "src", "build.rs", "notes.md")
        self.git(root, "commit", "-q", "-m", "fixture source")
        binary = target / "debug" / "cache_identity_fixture"

        def build() -> tuple[str, bool]:
            result = cargo("build", "--locked", "--offline", "-j", "2", "--message-format", "json")
            artifact = next(json.loads(line) for line in result.stdout.splitlines()
                            if '"compiler-artifact"' in line and '"cache_identity_fixture"' in line
                            and '"bin"' in line)
            return subprocess.check_output([str(binary)], text=True).strip(), artifact["fresh"]

        return build

    def test_deleted_tracked_file_does_not_recompile_every_build(self) -> None:
        build = self.short_hash_crate(self.repo)
        sha = self.git(self.repo, "rev-parse", "HEAD")
        short = self.git(self.repo, "rev-parse", "--short", "HEAD")
        self.assertEqual(build()[0], f"{sha} {short}")
        (self.repo / "src/retired.rs").unlink()
        self.assertEqual(build()[0], f"{sha} {short}-dirty")
        text, fresh = build()
        self.assertEqual(text, f"{sha} {short}-dirty")
        self.assertTrue(fresh, "an unchanged tree with a deleted tracked file recompiled the crate again")

    def test_detached_worktree_commit_refreshes_embedded_hash(self) -> None:
        worktree = self.directory / "detached"
        self.git(self.repo, "worktree", "add", "--detach", "-q", str(worktree), "HEAD")
        build = self.short_hash_crate(worktree)
        sha = self.git(worktree, "rev-parse", "HEAD")
        short = self.git(worktree, "rev-parse", "--short", "HEAD")
        self.assertEqual(build(), (f"{sha} {short}", False))
        self.assertTrue(build()[1], "unchanged detached worktree recompiled")
        self.git(worktree, "commit", "--allow-empty", "-q", "-m", "next")
        sha = self.git(worktree, "rev-parse", "HEAD")
        short = self.git(worktree, "rev-parse", "--short", "HEAD")
        self.assertEqual(build()[0], f"{sha} {short}")


if __name__ == "__main__":
    unittest.main()
