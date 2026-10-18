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
        self.assertNotIn(self.sha, output)
        self.assertNotIn("cargo:rerun-if-changed=", output)

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


if __name__ == "__main__":
    unittest.main()
