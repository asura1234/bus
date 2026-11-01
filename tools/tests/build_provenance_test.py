"""Exercise production identity collection through Cargo's real build cache."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]


@pytest.fixture
def identity_worktrees(tmp_path: Path):
    tmp_path = tmp_path.resolve()
    repo = tmp_path / "repo"
    repo.mkdir()
    hooks = tmp_path / "hooks"
    hooks.mkdir()
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("BUS_", "HERDR_", "GIT_"))}
    for key in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "LLVM_PROFILE_FILE"):
        env.pop(key, None)
    env.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1",
               CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="3")
    base = Path(env.get("CARGO_TARGET_DIR", str(tmp_path / "target"))).resolve()
    target = base / "build-provenance-fixtures" / tmp_path.name
    env["CARGO_TARGET_DIR"] = str(target)

    def git(checkout: Path, *args: str) -> str:
        return subprocess.check_output(
            ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
             "-c", "commit.gpgsign=false", "-c", f"core.hooksPath={hooks}", *args],
            cwd=checkout, env=env, text=True, stderr=subprocess.PIPE,
        ).strip()

    git(repo, "init", "-q", "-b", "fixture", f"--template={hooks}")
    (repo / ".cargo").mkdir()
    shutil.copyfile(ROOT / ".cargo/config.toml", repo / ".cargo/config.toml")
    shutil.copyfile(ROOT / "build_identity.rs", repo / "build_identity.rs")
    (repo / "Cargo.toml").write_text(
        '[package]\nname="bus_build_provenance_fixture"\nversion="0.0.0"\nedition="2021"\n'
        '[features]\nalternate=[]\n'
    )
    (repo / "build.rs").write_text(
        'include!("build_identity.rs");\nfn main() { record_build_identity(); }\n'
    )
    (repo / "src").mkdir()
    (repo / "src/main.rs").write_text(
        'fn main() { println!("{}|{}|{}", env!("BUS_BUILD_CHECKOUT"), '
        'env!("BUS_BUILD_SHA"), env!("BUS_BUILD_SHORT_SHA")); }\n'
    )
    subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=repo, env=env,
                   check=True, capture_output=True)
    git(repo, "add", ".cargo", "Cargo.toml", "Cargo.lock", "build.rs", "build_identity.rs", "src")
    git(repo, "commit", "-qm", "identical source A")
    sha_a = git(repo, "rev-parse", "HEAD")
    git(repo, "commit", "--allow-empty", "-qm", "identical source B")
    sha_b = git(repo, "rev-parse", "HEAD")
    a, b = tmp_path / "a", tmp_path / "b"
    git(repo, "worktree", "add", "-qb", "fixture-a", str(a), sha_a)
    git(repo, "worktree", "add", "-qb", "fixture-b", str(b), sha_b)
    for checkout in (a, b):
        # A checkout's newly written source mtimes must not hide a missing
        # provenance dependency by incidentally invalidating the build script.
        for path in checkout.rglob("*"):
            if path.is_file() and path.name != ".git":
                os.utime(path, (1_600_000_000, 1_600_000_000))

    def build(checkout: Path, *, alternate: bool = False) -> bool:
        args = ["cargo", "build", "--locked", "--offline", "--message-format=json"]
        if alternate:
            args.extend(["--features", "alternate"])
        result = subprocess.run(args, cwd=checkout, env=env, text=True,
                                capture_output=True, check=True)
        artifact = next(item for line in result.stdout.splitlines()
                        if (item := json.loads(line)).get("reason") == "compiler-artifact"
                        and item.get("executable"))
        actual = subprocess.check_output([artifact["executable"]], text=True).strip()
        sha = git(checkout, "rev-parse", "HEAD")
        short = git(checkout, "rev-parse", "--short", "HEAD")
        assert actual == f"{checkout}|{sha}|{short}", result.stderr
        return artifact["fresh"]

    try:
        yield repo, a, b, git, build
    finally:
        shutil.rmtree(target, ignore_errors=True)


def test_identical_sources_switch_checkout_and_head_without_cleaning(identity_worktrees):
    repo, a, b, git, build = identity_worktrees
    for checkout, alternate in ((a, False), (b, False), (a, False), (b, True), (a, True)):
        assert not build(checkout, alternate=alternate)
    assert build(a, alternate=True), "unchanged checkout should stay cached"
    git(a, "commit", "--allow-empty", "-qm", "advance branch without editing sources")
    assert not build(a, alternate=True)
    assert build(a, alternate=True)
    git(repo, "pack-refs", "--all", "--prune")
    build(a, alternate=True)
    assert build(a, alternate=True)
    git(a, "checkout", "--detach", "-q", "HEAD~1")
    assert not build(a, alternate=True)
    assert build(a, alternate=True)


def test_gitdir_pointer_change_refreshes_identity_without_source_edits(identity_worktrees):
    _, a, b, _, build = identity_worktrees
    build(a)
    assert build(a)
    # Worktree Git metadata can change independently of its checkout path.
    # Keep the previous gitdir alive so its old HEAD cannot cause a rebuild.
    marker = a / ".git"
    original = marker.read_text()
    try:
        marker.write_text((b / ".git").read_text())
        assert not build(a)
        assert build(a)
    finally:
        marker.write_text(original)


def test_common_gitdir_pointer_change_refreshes_identity(identity_worktrees, tmp_path):
    repo, a, _, git, build = identity_worktrees
    common = tmp_path / "other-common.git"
    git(repo, "clone", "--bare", "-q", str(repo), str(common))
    git(repo, f"--git-dir={common}", "update-ref", "refs/heads/fixture-a", "refs/heads/fixture-b")
    build(a)
    assert build(a)
    gitdir = Path(git(a, "rev-parse", "--absolute-git-dir"))
    marker = gitdir / "commondir"
    original = marker.read_text()
    try:
        marker.write_text(str(common.resolve()) + "\n")
        assert not build(a)
        assert build(a)
    finally:
        marker.write_text(original)
