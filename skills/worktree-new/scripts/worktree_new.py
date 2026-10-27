#!/usr/bin/env python3
"""Create an isolated checkout and prove readiness with its headless smoke recipe."""

from __future__ import annotations

import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys

from repo_profile import atomic_json, exclude_artifacts, git, main_worktree, scan
from repo_inference import governing_node_pin, numeric_node_pin, node_workspace_owner

GIB = 2**30


def create(repo: Path, branch: str, destination: Path, base: str | None = None) -> Path:
    repo = main_worktree(repo)
    destination = destination.expanduser().absolute()
    git(repo, "check-ref-format", "--branch", branch)
    if branch in {"main", "master"} or branch.startswith("-"):
        raise ValueError("choose a feature branch")
    if destination.exists():
        registered = git(repo, "worktree", "list", "--porcelain", "-z").split("\0")
        if "worktree " + str(destination.resolve()) not in registered:
            raise ValueError("destination exists but is not a registered worktree")
        if git(destination, "branch", "--show-current") != branch:
            raise ValueError(
                "existing worktree has a different branch; leave it unchanged"
            )
        return destination
    if base is None:
        advertised = git(repo, "ls-remote", "--symref", "origin", "HEAD")
        default = next(
            (
                line.split()[1].removeprefix("refs/heads/")
                for line in advertised.splitlines()
                if line.startswith("ref: refs/heads/")
            ),
            None,
        )
        if not default:
            raise ValueError("origin has no symbolic HEAD; supply --base explicitly")
        git(
            repo,
            "fetch",
            "origin",
            f"+refs/heads/{default}:refs/remotes/origin/{default}",
        )
        git(repo, "remote", "set-head", "origin", default)
        base = "origin/" + default
    else:
        git(repo, "fetch", "origin")
    git(repo, "rev-parse", "--verify", base + "^{commit}")
    local = git(repo, "branch", "--list", branch)
    remote = git(repo, "ls-remote", "--heads", "origin", "refs/heads/" + branch)
    if local:
        git(repo, "worktree", "add", str(destination), branch)
    elif remote:
        git(
            repo, "fetch", "origin", f"refs/heads/{branch}:refs/remotes/origin/{branch}"
        )
        git(
            repo,
            "worktree",
            "add",
            "--track",
            "-b",
            branch,
            str(destination),
            "origin/" + branch,
        )
    else:
        git(repo, "worktree", "add", "--no-track", "-b", branch, str(destination), base)
    return destination


def free_space(repo: Path) -> float:
    free = shutil.disk_usage(repo).free / GIB
    if free < 25:
        raise ValueError(
            f"need at least 25 GiB free before provisioning; have {free:.2f} GiB"
        )
    return free


def child_env(repo: Path, tool_bins: list[Path]) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if not k.startswith(("BUS_", "HERDR_"))}
    bins = [str(Path(p).expanduser().resolve()) for p in tool_bins]
    bins.append(str(Path.home() / ".cargo/bin"))
    env["PATH"] = os.pathsep.join([*bins, env.get("PATH", "")])
    env["CI"] = "true"
    env["CARGO_INCREMENTAL"] = "0"
    cap = max(1, ((os.cpu_count() or 2) - 1) // 2)
    requested = env.get("CARGO_BUILD_JOBS")
    if requested is not None:
        try:
            jobs = int(requested)
        except ValueError as error:
            raise ValueError("CARGO_BUILD_JOBS must be a positive integer") from error
        if jobs < 1:
            raise ValueError("CARGO_BUILD_JOBS must be a positive integer")
        cap = min(cap, jobs)
    env["CARGO_BUILD_JOBS"] = str(cap)
    env.setdefault("CARGO_TARGET_DIR", str(repo / "target"))
    return env


def verify_node(repo: Path, profile: dict, env: dict, cwd=".", tool_bins=()) -> None:
    tool = governing_node_pin(profile["toolchains"], cwd)
    if not tool:
        return
    if not numeric_node_pin(tool["pin"]):
        raise ValueError(f"{tool['path']}: resolve Node alias to a numeric pin")
    candidates = [None, *tool_bins]
    actual = "unavailable"
    for directory in candidates:
        candidate = env.copy()
        if directory is not None:
            candidate["PATH"] = (
                str(Path(directory).expanduser().resolve()) + os.pathsep + env["PATH"]
            )
        try:
            result = subprocess.run(
                ["node", "--version"],
                cwd=repo / cwd,
                env=candidate,
                text=True,
                capture_output=True,
            )
        except FileNotFoundError:
            continue
        actual = result.stdout.strip().removeprefix("v")
        if result.returncode == 0 and actual.split(".")[
            : len(tool["pin"].split("."))
        ] == tool["pin"].split("."):
            env["PATH"] = candidate["PATH"]
            return
    raise ValueError(
        f"{tool['path']} requires Node {tool['pin']}; found {actual}. Supply --tool-bin for this pin"
    )


def share_dependencies(repo: Path, profile: dict) -> set[int]:
    main = Path(profile["main_worktree"])
    allowed = set(profile["recipe"]["shared_dependencies"])
    skips = set()
    for name in allowed:
        package = next(
            (
                p
                for p in profile["package_managers"]
                if (Path(p["path"]).parent / "node_modules").as_posix() == name
            ),
            None,
        )
        if (
            not package
            or not package["lockfile"]
            or node_workspace_owner(profile["package_managers"], Path(name).parent)
        ):
            raise ValueError(
                f"shared dependencies require a lock-owning package root: {name}"
            )
        source = (main / name).resolve()
        if source.is_dir():
            for root, directories, files in os.walk(source, followlinks=False):
                for child in directories + files:
                    link = Path(root) / child
                    if link.is_symlink() and not link.resolve().is_relative_to(source):
                        raise ValueError(
                            f"dependency link resolves outside node_modules (workspace/source link): {link}"
                        )
        owner = Path(name).parent
        for index, item in enumerate(profile["recipe"]["prepare"]):
            cwd = Path(item["cwd"])
            if not (owner.is_relative_to(cwd) or cwd.is_relative_to(owner)):
                continue
            argv = item["argv"]
            if argv == ["cargo", "fetch", "--locked"]:
                continue
            args = argv[1:] if Path(argv[0]).stem == "corepack" else argv
            manager = Path(args[0]).stem if args else ""
            verb = (
                args[1] if len(args) > 1 else ("install" if manager == "yarn" else "")
            )
            if (
                cwd == owner
                and manager in {"npm", "pnpm", "yarn", "bun"}
                and verb in {"install", "ci", "i"}
            ):
                skips.add(index)
            else:
                raise ValueError(
                    f"prepare command may write shared dependencies; use a private workspace/install: {argv}"
                )
    for name in allowed:
        if not any(
            (Path(p["path"]).parent / "node_modules").as_posix() == name
            for p in profile["package_managers"]
        ):
            raise ValueError(f"shared dependency has no package manifest: {name}")
        # A shared dependency graph is valid only for identical Node manifests and locks.
        names = [
            m["path"]
            for m in profile["manifests"]
            if Path(m["path"]).name
            in {
                "package.json",
                "package-lock.json",
                "npm-shrinkwrap.json",
                "pnpm-lock.yaml",
                "pnpm-workspace.yaml",
                "yarn.lock",
                "bun.lock",
                "bun.lockb",
            }
        ]
        if any(
            not (main / p).is_file()
            or (repo / p).read_bytes() != (main / p).read_bytes()
            for p in names
        ):
            raise ValueError(
                "cannot reuse shared dependencies with different package manifests or lockfiles"
            )
        source, dest = main / name, repo / name
        if not source.is_dir():
            raise ValueError(f"main worktree shared dependency missing: {source}")
        if repo == main:
            continue
        if dest.is_symlink():
            if dest.resolve() != source.resolve():
                raise ValueError(f"wrong dependency symlink: {dest}")
        elif dest.exists():
            raise ValueError(
                f"refusing to replace existing dependency directory: {dest}"
            )
        else:
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.symlink_to(source.resolve(), target_is_directory=True)
    for package in profile["package_managers"]:
        name = (Path(package["path"]).parent / "node_modules").as_posix()
        if (repo / name).is_symlink() and name not in allowed:
            raise ValueError(
                f"{name} is a symlink; opt in with shared_dependencies before provisioning"
            )
    # Even a repo without package.json can have a dangerous dependency symlink.
    if (repo / "node_modules").is_symlink() and "node_modules" not in allowed:
        raise ValueError("node_modules is a symlink; configure shared_dependencies")
    return skips


def execute(repo: Path, item: dict, env: dict) -> None:
    cwd = (repo / item["cwd"]).resolve()
    if not cwd.is_relative_to(repo) or not cwd.is_dir():
        raise ValueError(f"command cwd escapes or is missing: {item['cwd']}")
    argv = item["argv"]
    print("+ " + json.dumps(item), flush=True)
    process = subprocess.Popen(
        (["nice", "-n", "19"] if os.name == "posix" else []) + argv,
        cwd=cwd,
        env=env,
        start_new_session=os.name == "posix",
    )
    try:
        code = process.wait()
    except BaseException:
        if os.name == "posix":
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        else:
            process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            if os.name == "posix":
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
            process.wait()
        raise
    if code:
        raise ValueError(f"command exited {code}: {argv}")


@contextmanager
def interruption():
    def stop(signum, _frame):
        raise InterruptedError(f"provisioning interrupted by signal {signum}")

    signals = [signal.SIGINT, signal.SIGTERM]
    signals.extend(
        getattr(signal, name) for name in ("SIGHUP", "SIGQUIT") if hasattr(signal, name)
    )
    previous = {sig: signal.signal(sig, stop) for sig in signals}
    try:
        yield
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def provision(
    repo: Path, *, build_slot: Path, tool_bins: list[Path] | None = None
) -> dict:
    with interruption():
        return _provision(repo, build_slot=build_slot, tool_bins=tool_bins)


def _provision(
    repo: Path, *, build_slot: Path, tool_bins: list[Path] | None = None
) -> dict:
    repo = Path(git(repo.resolve(), "rev-parse", "--show-toplevel")).resolve()
    evidence_path = repo / "temp/.bus/worktree-ready.json"
    evidence_path.unlink(missing_ok=True)
    evidence = {
        "schema_version": 1,
        "head": git(repo, "rev-parse", "HEAD"),
        "manifest_hash": "",
        "status": "failed",
        "commands": [],
        "free_gib": 0,
        "error": None,
    }
    acquired = False
    try:
        if git(repo, "status", "--porcelain", "--untracked-files=no"):
            raise ValueError(
                "tracked worktree content changed; commit before provisioning"
            )
        profile = scan(repo)
        evidence["manifest_hash"] = profile["manifest_hash"]
        evidence["free_gib"] = free_space(repo)
        count = sum(
            line.startswith("worktree ")
            for line in git(repo, "worktree", "list", "--porcelain").splitlines()
        )
        print(
            f"Disk estimate: {profile['recipe']['disk_estimate_gib']} GiB; free: {evidence['free_gib']:.2f} GiB; existing worktrees: {count}",
            flush=True,
        )
        for hazard in profile["hazards"]:
            print(
                f"WARNING {hazard['id']}: {hazard['reason']} {hazard['action']}",
                flush=True,
            )
        build_slot = Path(build_slot).expanduser().absolute()
        build_slot.mkdir()  # atomic admission; never remove a slot owned by someone else
        acquired = True
        (build_slot / "owner").write_text(f"pid={os.getpid()}\nrepo={repo}\n")
        env = child_env(repo, tool_bins or [])
        if (
            profile["submodules"]
            and profile["policy"]["submodule_update"] == "recursive"
        ):
            item = {
                "cwd": ".",
                "argv": ["git", "submodule", "update", "--init", "--recursive"],
            }
            free_space(repo)
            evidence["commands"].append(item)
            execute(repo, item, env)
            profile = scan(
                repo
            )  # discover the actual pinned toolchains after initialization
            evidence["manifest_hash"] = profile["manifest_hash"]
        if profile["recipe"]["unresolved"]:
            raise ValueError("; ".join(profile["recipe"]["unresolved"]))
        if not profile["recipe"]["smoke"]:
            raise ValueError("no headless smoke recipe; cannot report ready")
        for name in profile["recipe"]["private_paths"]:
            if not (repo / name).resolve().is_relative_to(repo):
                raise ValueError(
                    f"private runtime state must stay inside this worktree: {name}"
                )
        skipped = share_dependencies(repo, profile)
        for stage in ("prepare", "smoke"):
            for index, item in enumerate(profile["recipe"][stage]):
                if stage == "prepare" and index in skipped:
                    continue
                free_space(repo)
                step_env = env.copy()
                verify_node(repo, profile, step_env, item["cwd"], tool_bins or [])
                evidence["commands"].append(item)
                execute(repo, item, step_env)
        if (
            git(repo, "rev-parse", "HEAD") != evidence["head"]
            or git(repo, "status", "--porcelain", "--untracked-files=no")
            or scan(repo)["manifest_hash"] != evidence["manifest_hash"]
        ):
            raise ValueError(
                "HEAD, tracked content, or manifests changed during provisioning"
            )
        evidence["status"] = "ready"
    except (OSError, ValueError) as error:
        evidence["error"] = str(error)
        raise ValueError(str(error)) from error
    finally:
        try:
            exclude_artifacts(repo)
            atomic_json(evidence_path, evidence)
        finally:
            if acquired:
                (build_slot / "owner").unlink()
                build_slot.rmdir()
    return evidence


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["create", "provision"])
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--branch")
    parser.add_argument("--path", type=Path)
    parser.add_argument("--base")
    parser.add_argument("--build-slot", type=Path, required=True)
    parser.add_argument("--tool-bin", type=Path, action="append", default=[])
    args = parser.parse_args()
    try:
        repo = args.repo.resolve()
        if args.action == "create":
            if not args.branch:
                raise ValueError("--branch is required for create")
            main = main_worktree(repo)
            destination = args.path or main.parent / (
                main.name + "-" + args.branch.replace("/", "-")
            )
            free_space(main)
            repo = create(repo, args.branch, destination, args.base)
        result = provision(repo, build_slot=args.build_slot, tool_bins=args.tool_bin)
        print(
            f"Worktree ready: {repo}\nHead: {result['head']}\nEvidence: {repo / 'temp/.bus/worktree-ready.json'}"
        )
    except (OSError, ValueError) as error:
        print(f"worktree-new: NOT READY: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
