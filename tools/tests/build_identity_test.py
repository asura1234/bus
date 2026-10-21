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
            f"include!({json.dumps((ROOT / 'build.rs').as_posix())});\n"
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
        self.git(self.repo, "init", "-q", "-b", "fixture", f"--template={self.hooks}")
        self.git(self.repo, "commit", "--allow-empty", "-q", "-m", "fixture")
        self.sha = self.git(self.repo, "rev-parse", "HEAD")

    def record(self, manifest: Path, extra: dict[str, str] | None = None) -> str:
        return subprocess.check_output(
            [str(self.binary)],
            cwd=manifest,
            env={**self.environment, **(extra or {}), "CARGO_MANIFEST_DIR": str(manifest), "PROFILE": "fixture"},
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
        source = self.repo / "tracked.txt"
        source.write_text("clean\n")
        self.git(self.repo, "add", "tracked.txt")
        self.git(self.repo, "commit", "-q", "-m", "source")
        short = self.git(self.repo, "rev-parse", "--short", "HEAD")
        self.assertIn(f"cargo:rustc-env=BUS_BUILD_SHORT_SHA={short}\n", self.record(self.repo))
        source.write_text("dirty\n")
        self.assertIn(f"cargo:rustc-env=BUS_BUILD_SHORT_SHA={short}-dirty\n", self.record(self.repo))
        self.git(self.repo, "restore", "tracked.txt")
        (self.repo / "untracked.txt").write_text("new source\n")
        self.assertIn(f"cargo:rustc-env=BUS_BUILD_SHORT_SHA={short}-dirty\n", self.record(self.repo))

    def test_missing_git_uses_unknown_short_hash(self) -> None:
        output = self.record(self.repo, {"PATH": str(self.hooks)})
        self.assertIn("cargo:rustc-env=BUS_BUILD_SHORT_SHA=unknown\n", output)

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
        )
        (self.repo / "src").mkdir()
        (self.repo / "src/main.rs").write_text(
            'fn main() { println!("{} {}", env!("BUS_BUILD_SHA"), '
            'option_env!("BUS_BUILD_SHORT_SHA").unwrap_or("missing")); }\n'
        )
        (self.repo / "build.rs").write_text(
            "mod production {\n"
            f"include!({json.dumps((ROOT / 'build.rs').as_posix())});\n"
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

        (self.repo / "tracked.txt").write_text("clean\n")
        self.git(self.repo, "add", "Cargo.toml", "Cargo.lock", "src/main.rs", "build.rs", "tracked.txt")
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

        (self.repo / "tracked.txt").write_text("changed\n")
        build(next_sha, dirty=True)
        build(next_sha, cached=True, dirty=True)
        self.git(self.repo, "add", "tracked.txt")
        build(next_sha, dirty=True)
        self.git(self.repo, "restore", "--staged", "--worktree", "tracked.txt")
        build(next_sha)
        build(next_sha, cached=True)


if __name__ == "__main__":
    unittest.main()
