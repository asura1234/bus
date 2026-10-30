from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def test_shared_target_switches_checkout_identity_and_local_dependency(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    hooks = tmp_path / "hooks"
    hooks.mkdir()
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("BUS_", "HERDR_", "GIT_"))}
    for key in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "LLVM_PROFILE_FILE"):
        env.pop(key, None)
    env["CARGO_INCREMENTAL"] = "0"
    env["CARGO_BUILD_JOBS"] = "3"

    def git(*args: str) -> str:
        return subprocess.check_output(
            ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
             "-c", "commit.gpgsign=false", "-c", f"core.hooksPath={hooks}", *args],
            cwd=repo, env=env, text=True, stderr=subprocess.PIPE,
        ).strip()

    git("init", "-q", "-b", "fixture", f"--template={hooks}")
    (repo / ".cargo").mkdir()
    shutil.copyfile(ROOT / ".cargo/config.toml", repo / ".cargo/config.toml")
    (repo / "src").mkdir()
    (repo / "vendor").mkdir()
    shutil.copytree(ROOT / "vendor/portable-pty", repo / "vendor/portable-pty")
    manifest = repo / "vendor/portable-pty/Cargo.toml"
    manifest.write_text(manifest.read_text().replace('name = "portable-pty"',
                                                     'name = "provenance-probe-pty"', 1))
    source = repo / "vendor/portable-pty/src/lib.rs"
    source.write_text(source.read_text() + '\npub fn provenance_probe() -> &\'static str { "A" }\n')
    (repo / "Cargo.toml").write_text(
        '[package]\nname="bus_provenance_regression_29280"\nversion="0.1.0"\nedition="2021"\n'
        '[features]\nalternate=[]\n'
        '[dependencies]\nportable-pty={package="provenance-probe-pty",path="vendor/portable-pty"}\n'
    )
    (repo / "src/main.rs").write_text(
        'fn main() { println!("{}|{}|{}", env!("CARGO_MANIFEST_DIR"), '
        'env!("BUS_BUILD_SHA"), portable_pty::provenance_probe()); }\n'
    )
    (repo / "build.rs").write_text(
        f'include!({json.dumps((ROOT / "build_identity.rs").as_posix())});\n'
        'fn main() { record_build_identity(); }\n'
    )
    git("add", ".cargo/config.toml", "Cargo.toml", "src", "vendor", "build.rs")
    git("commit", "-qm", "fixture A")
    sha_a = git("rev-parse", "HEAD")
    source.write_text(source.read_text().replace(' { "A" }', ' { "B" }'))
    git("add", "vendor/portable-pty/src/lib.rs")
    git("commit", "-qm", "fixture B")
    sha_b = git("rev-parse", "HEAD")
    a, b = tmp_path / "a", tmp_path / "b"
    git("worktree", "add", "-qb", "fixture-a", str(a), sha_a)
    git("worktree", "add", "-qb", "fixture-b", str(b), sha_b)

    target = Path(env.setdefault("CARGO_TARGET_DIR", str(tmp_path / "target")))
    binary = target / "debug" / ("bus_provenance_regression_29280.exe" if os.name == "nt" else "bus_provenance_regression_29280")

    def build(checkout: Path, sha: str, value: str, *, alternate: bool = False) -> bool:
        args = ["cargo", "build", "--offline", "--message-format=json"]
        if alternate:
            args.extend(["--features", "alternate"])
        result = subprocess.run(
            args, cwd=checkout, env=env,
            text=True, capture_output=True, check=True,
        )
        artifact = next(json.loads(line) for line in result.stdout.splitlines()
                        if '"compiler-artifact"' in line and '"bus_provenance_regression_29280"' in line
                        and '"bin"' in line)
        actual = subprocess.check_output([str(binary)], text=True).strip()
        assert actual == f"{checkout}|{sha}|{value}"
        return artifact["fresh"]

    build(a, sha_a, "A")
    build(b, sha_b, "B")
    build(a, sha_a, "A")
    build(b, sha_b, "B", alternate=True)
    build(a, sha_a, "A")
    assert build(a, sha_a, "A"), "unchanged checkout should remain fresh"

    # Cargo discovers configuration from cwd, not --manifest-path. Fail closed
    # if the checkout marker is unavailable rather than reusing cached output.
    outside = subprocess.run(
        ["cargo", "build", "--offline", "--manifest-path", str(a / "Cargo.toml")],
        cwd=tmp_path, env=env, text=True, capture_output=True,
    )
    assert outside.returncode != 0
    assert "BUS_CHECKOUT_ROOT" in outside.stderr
