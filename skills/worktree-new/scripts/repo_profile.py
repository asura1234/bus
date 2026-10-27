#!/usr/bin/env python3
"""Discover a checkout and atomically cache the shared repository profile."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import tomllib

NAMES = {
    "package.json",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    ".node-version",
    ".nvmrc",
    "CMakeLists.txt",
    "CMakePresets.json",
    "Makefile",
    "makefile",
    "justfile",
    "Justfile",
    "run",
    ".gitmodules",
    "Podfile",
    "Podfile.lock",
    "project.pbxproj",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "gradle.properties",
    "gradle-wrapper.properties",
    "gradlew",
    "media-runtime-lock.json",
}
LOCKS = {
    "npm": ("npm-shrinkwrap.json", "package-lock.json"),
    "pnpm": ("pnpm-lock.yaml",),
    "yarn": ("yarn.lock",),
    "bun": ("bun.lock", "bun.lockb"),
}
SKIP = {
    "node_modules",
    "target",
    "build",
    "dist",
    ".git",
    ".next",
    "temp",
    "Pods",
    ".gradle",
}


def git(repo: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args], capture_output=True, text=True
    )
    if result.returncode:
        raise ValueError(f"git {' '.join(args)}: {result.stderr.strip()}")
    return result.stdout.rstrip("\n")


def main_worktree(repo: Path) -> Path:
    records = git(repo, "worktree", "list", "--porcelain", "-z").split("\0")
    return Path(records[0].removeprefix("worktree ")).resolve()


def relative(value: object) -> str:
    if not isinstance(value, str) or not value or "\\" in value:
        raise ValueError("expected normalized relative path")
    path = Path(value)
    if path.is_absolute() or ".." in path.parts or path.as_posix() != value:
        raise ValueError("expected normalized relative path")
    return value


def commands(value: object) -> list[dict]:
    if not isinstance(value, list):
        raise ValueError("commands must be a list")
    for command in value:
        if not isinstance(command, dict) or set(command) != {"cwd", "argv"}:
            raise ValueError("command requires exactly cwd and argv")
        relative(command["cwd"])
        argv = command["argv"]
        if (
            not isinstance(argv, list)
            or not argv
            or any(not isinstance(a, str) or not a or "\0" in a for a in argv)
        ):
            raise ValueError("argv must be nonempty strings")
    return value


def keys(value: object, allowed: set[str]) -> dict:
    if not isinstance(value, dict) or set(value) - allowed:
        raise ValueError(f"unknown keys or invalid table; expected {sorted(allowed)}")
    return value


def overrides(repo: Path) -> dict:
    path = repo / ".bus/repo.toml"
    if path.is_symlink() or not path.resolve().is_relative_to(repo.resolve()):
        raise ValueError("override path escapes checkout or is a symlink")
    if not path.exists():
        return {}
    data = keys(
        tomllib.loads(path.read_text()),
        {
            "gate_command",
            "commit_style",
            "require_exact_head_gate",
            "submodules",
            "worktree",
        },
    )
    if type(data.get("require_exact_head_gate", False)) is not bool:
        raise ValueError("require_exact_head_gate must be boolean")
    for name in ("gate_command", "commit_style"):
        if name in data and (not isinstance(data[name], str) or not data[name].strip()):
            raise ValueError(f"{name} must be a nonempty string")
    sub = keys(data.get("submodules", {}), {"update"})
    if sub.get("update", "recursive") not in {"recursive", "none"}:
        raise ValueError("submodules.update must be recursive or none")
    wt = keys(
        data.get("worktree", {}),
        {"prepare", "smoke", "disk_estimate_gib", "shared_dependencies"},
    )
    for name in ("prepare", "smoke"):
        if name in wt:
            commands(wt[name])
    if "smoke" in wt and not wt["smoke"]:
        raise ValueError("smoke must include a build and headless run/test")
    if "disk_estimate_gib" in wt:
        estimate = wt["disk_estimate_gib"]
        if type(estimate) not in (int, float) or not 0 < estimate < 100000:
            raise ValueError("disk_estimate_gib must be positive and finite")
    shared = wt.get("shared_dependencies", [])
    if not isinstance(shared, list) or len(set(shared)) != len(shared):
        raise ValueError("shared_dependencies must be a unique list")
    for path in shared:
        if Path(relative(path)).name != "node_modules":
            raise ValueError(
                "only node_modules may be shared; build/deps contains writable runtime state"
            )
    return data


def submodules(repo: Path, prefix: str = "") -> list[dict]:
    if not (repo / ".gitmodules").is_file():
        return []
    config = git(
        repo,
        "config",
        "-z",
        "--file",
        ".gitmodules",
        "--get-regexp",
        r"^submodule\..*\.path$",
    )
    result = []
    for item in filter(None, config.split("\0")):
        key, path = item.split("\n", 1)
        relative(path)
        full = repo / path
        url = git(repo, "config", "--file", ".gitmodules", "--get", key[:-4] + "url")
        entry = git(repo, "ls-files", "--stage", "--", path).split()
        initialized = (full / ".git").exists()
        result.append(
            {
                "path": prefix + path,
                "url": url,
                "commit": entry[1] if len(entry) >= 2 else None,
                "initialized": initialized,
            }
        )
        if initialized:
            result.extend(submodules(full, prefix + path + "/"))
    return result


def input_paths(repo: Path, subs: list[dict]) -> list[str]:
    paths = set()
    if (repo / ".bus/repo.toml").exists():
        paths.add(".bus/repo.toml")
    roots = [(repo, "")] + [
        (repo / s["path"], s["path"] + "/") for s in subs if s["initialized"]
    ]
    for root, prefix in roots:
        for name in git(
            root, "ls-files", "--cached", "--others", "--exclude-standard", "-z"
        ).split("\0"):
            if not name:
                continue
            path = Path(name)
            if any(part in SKIP for part in path.parts[:-1]):
                continue
            if path.name not in NAMES and name != ".bus/repo.toml":
                continue
            full = root / name
            if full.is_symlink() or not full.resolve().is_relative_to(repo.resolve()):
                raise ValueError(
                    f"manifest path escapes checkout or is a symlink: {prefix + name}"
                )
            if full.is_file():
                paths.add(prefix + name)
    return sorted(paths)


def command(argv: list[str], cwd: str = ".") -> dict:
    return {"cwd": cwd, "argv": argv}


def package_info(repo: Path, paths: list[str]) -> list[dict]:
    result = []
    for name in paths:
        if Path(name).name != "package.json":
            continue
        data = json.loads((repo / name).read_text())
        parent = Path(name).parent
        found = [
            (manager, parent / lock)
            for manager, locks in LOCKS.items()
            for lock in locks
            if (parent / lock).as_posix() in paths
        ]
        managers = {manager for manager, _ in found}
        declared = data.get("packageManager")
        version = None
        manager = next(iter(managers), "npm")
        if declared:
            match = re.fullmatch(r"(npm|pnpm|yarn|bun)@(.+)", declared)
            if not match:
                raise ValueError(f"unsupported packageManager in {name}")
            manager, version = match.groups()
        if len(managers) > 1 or (managers and manager not in managers):
            raise ValueError(f"conflicting lockfiles in {parent}")
        workspaces = data.get("workspaces", [])
        if isinstance(workspaces, dict):
            workspaces = workspaces.get("packages", [])
        if not isinstance(workspaces, list) or any(
            not isinstance(p, str) for p in workspaces
        ):
            raise ValueError(f"invalid workspaces in {name}")
        workspace_file = repo / parent / "pnpm-workspace.yaml"
        if not workspaces and workspace_file.is_file():
            # Read the simple block-list form without introducing a YAML dependency.
            # More complex YAML is not guessed; package.json can supply the patterns.
            section = False
            for line in workspace_file.read_text().splitlines():
                if re.match(r"^packages:\s*(?:#.*)?$", line):
                    section = True
                elif section and re.match(r"^\s+-\s+", line):
                    words = shlex.split(re.sub(r"^\s+-\s+", "", line), comments=True)
                    if len(words) != 1:
                        raise ValueError(
                            "unsupported workspace YAML; use package.json workspaces"
                        )
                    workspaces.append(words[0])
                elif (
                    line.strip()
                    and not line.lstrip().startswith("#")
                    and not line[0].isspace()
                ):
                    if line.startswith("packages:"):
                        raise ValueError(
                            "unsupported workspace YAML; use package.json workspaces"
                        )
                    section = False
        result.append(
            {
                "path": name,
                "manager": manager,
                "version": version,
                "lockfile": found[0][1].as_posix() if found else None,
                "workspaces": workspaces,
            }
        )
    return result


def inventory(repo: Path, paths: list[str]) -> tuple[list[dict], list[dict]]:
    tools, entries = [], []
    for name in paths:
        path = Path(name)
        text = (repo / name).read_text(errors="replace")
        kind, pin = None, None
        if path.name in {"rust-toolchain", "rust-toolchain.toml"}:
            kind = "rust"
            pin = (
                tomllib.loads(text).get("toolchain", {}).get("channel")
                if path.suffix == ".toml"
                else text.strip()
            )
        elif path.name in {".node-version", ".nvmrc"}:
            kind, pin = "node", text.strip().removeprefix("v")
        elif path.name == "CMakeLists.txt":
            kind = "cmake"
        elif path.name == "project.pbxproj":
            kind = "xcode"
        elif path.name in {
            "build.gradle",
            "build.gradle.kts",
            "gradle-wrapper.properties",
        }:
            kind = "gradle"
            ndk = re.search(r'ndkVersion\s*(?:=\s*)?["\']([^"\']+)', text)
            if ndk:
                tools.append({"kind": "android-ndk", "path": name, "pin": ndk[1]})
        if kind:
            tools.append({"kind": kind, "path": name, "pin": pin})
        ekind, cmds = None, []
        if path.name == "Cargo.toml":
            ekind, cmds = "cargo", ["build", "test", "run"]
        elif path.name == "package.json":
            ekind, cmds = "package", sorted(json.loads(text).get("scripts", {}))
        elif path.name in {"Makefile", "makefile", "justfile", "Justfile"}:
            ekind = "make" if path.name.lower() == "makefile" else "just"
            cmds = re.findall(r"(?m)^([A-Za-z_][\w-]*)[^\n:=]*:(?!=)", text)
        elif path.name in {"run", "gradlew", "CMakeLists.txt", "project.pbxproj"}:
            ekind = {
                "run": "run",
                "gradlew": "gradle",
                "CMakeLists.txt": "cmake",
                "project.pbxproj": "xcode",
            }[path.name]
        if ekind:
            entries.append({"kind": ekind, "path": name, "commands": cmds})
    return sorted(tools, key=lambda t: (t["path"], t["kind"])), entries


def infer_recipe(
    repo: Path, paths: list[str], packages: list[dict], config: dict
) -> dict:
    prepare, smoke, unresolved = [], [], []
    cargo = [
        p
        for p in paths
        if Path(p).name == "Cargo.toml" and not p.startswith(("vendor/", "external/"))
    ]
    cargo = [
        p
        for p in cargo
        if not any(
            Path(other).parent in Path(p).parents for other in cargo if other != p
        )
    ]
    for name in cargo:
        cwd = Path(name).parent.as_posix()
        if (Path(cwd) / "Cargo.lock").as_posix() not in paths:
            unresolved.append(
                f"{name}: locked Cargo dependency resolution requires Cargo.lock"
            )
        prepare.append(command(["cargo", "fetch", "--locked"], cwd))
        smoke.extend(
            [
                command(["cargo", "build", "--locked"], cwd),
                command(["cargo", "test", "--locked"], cwd),
            ]
        )
    root_package = next((p for p in packages if p["path"] == "package.json"), None)
    if root_package:
        manager, version = root_package["manager"], root_package["version"]
        if not root_package["lockfile"]:
            unresolved.append(
                "package.json: a committed package-manager lockfile is required"
            )
        install = {
            "npm": ["ci"],
            "pnpm": ["install", "--frozen-lockfile"],
            "bun": ["install", "--frozen-lockfile"],
            "yarn": ["install", "--immutable"]
            if version and not version.startswith("1.")
            else ["install", "--frozen-lockfile"],
        }[manager]
        prepare.append(command([manager, *install]))
        scripts = json.loads((repo / "package.json").read_text()).get("scripts", {})
        test = next(
            (
                s
                for s in ("test:ci", "test")
                if s in scripts
                and not re.search(
                    r"\b(watch|electron|run-ios|run-android)\b", scripts[s]
                )
            ),
            None,
        )
        if "build" in scripts and test:
            smoke.extend(
                [command([manager, "run", "build"]), command([manager, "run", test])]
            )
        else:
            unresolved.append(
                "package.json: configure worktree.smoke with a build and non-GUI test/run"
            )
    if not cargo and not root_package and "CMakeLists.txt" in paths:
        smoke.extend(
            [
                command(["cmake", "-S", ".", "-B", "build"]),
                command(["cmake", "--build", "build"]),
                command(
                    [
                        "ctest",
                        "--test-dir",
                        "build",
                        "--no-tests=error",
                        "--output-on-failure",
                    ]
                ),
            ]
        )
    if not smoke:
        unresolved.append(
            "No headless build and test recipe inferred; configure worktree.smoke"
        )
    wt = config.get("worktree", {})
    if "smoke" in wt:
        smoke, unresolved = wt["smoke"], []
    if "prepare" in wt:
        prepare = wt["prepare"]
    targets = sorted(
        {
            (Path(p).parent / "target").as_posix()
            for p in paths
            if Path(p).name == "Cargo.toml" and not p.startswith("vendor/")
        }
    )
    nested_count = sum(p != "target" for p in targets)
    return {
        "prepare": prepare,
        "smoke": smoke,
        "unresolved": unresolved,
        "shared_dependencies": wt.get("shared_dependencies", []),
        "target_paths": targets,
        "disk_estimate_gib": wt.get("disk_estimate_gib", 15 + 4 * nested_count),
        "disk_estimate_basis": "15 GiB planning allowance; native Rust targets add 4 GiB each",
    }


def hazards(
    repo: Path, paths: list[str], subs: list[dict], tools: list[dict]
) -> list[dict]:
    result = []

    def add(identity, affected, reason, action):
        result.append(
            {"id": identity, "paths": affected, "reason": reason, "action": action}
        )

    if (repo / "build/deps").is_symlink() or any(
        Path(p).name == "media-runtime-lock.json" for p in paths
    ):
        add(
            "shared-deps",
            ["build/deps", "ci/deps/media-runtime-lock.json"],
            "Shared build/deps activation receipts carry a releaseId and are writable; another checkout can invalidate them.",
            "Do not share build/deps. Before a runtime-lock edit, privatize media-runtime and link only other children; re-import stale receipts before attributing a failure.",
        )
    if subs:
        local = subprocess.run(
            ["git", "-C", str(repo), "symbolic-ref", "--short", "HEAD"],
            capture_output=True,
            text=True,
        )
        branch = local.stdout.strip() or "<integration-base>"
        # Prefer the origin default; a local fixture without an origin uses its own branch.
        remote = subprocess.run(
            [
                "git",
                "-C",
                str(repo),
                "symbolic-ref",
                "--short",
                "refs/remotes/origin/HEAD",
            ],
            capture_output=True,
            text=True,
        )
        base = remote.stdout.strip() or "origin/" + branch
        comparisons = "; ".join(f"git ls-tree {base} {s['path']}" for s in subs)
        add(
            "fresh-gitlink",
            [s["path"] for s in subs],
            "A pinned gitlink may contain a real type gap hidden by ignored generated files in older checkouts.",
            f"On fresh-only failure compare {comparisons} with each upstream branch tip; run the gate itself (for TypeScript: pnpm exec tsc --noEmit --pretty false --incremental false). Do not copy generated files or reorder gates.",
        )
    pinned_subs = [
        t["path"]
        for t in tools
        if t["pin"] and any(t["path"].startswith(s["path"] + "/") for s in subs)
    ]
    if pinned_subs or any(not s["initialized"] for s in subs):
        add(
            "submodule-pins",
            pinned_subs,
            "Toolchain pins inside submodules can differ from root pins.",
            "Preserve every discovered pinned SDK/toolchain; Rust toolchains are outside cleanup scope.",
        )
    nodes = [t["path"] for t in tools if t["kind"] == "node"]
    if nodes:
        add(
            "node-abi",
            nodes,
            "A different Node ABI can fail native modules even when CI passes.",
            "Require the exact pinned Node on PATH for every command; supply --tool-bin if necessary.",
        )
    add(
        "disk-floor",
        [],
        "Builds and apps can fail before logging when disk is full.",
        "Check free space before every provisioning command; require at least 25 GiB.",
    )
    rerere = subprocess.run(
        ["git", "-C", str(repo), "config", "--bool", "--get", "rerere.enabled"],
        capture_output=True,
        text=True,
    )
    if rerere.stdout.strip() == "true":
        add(
            "rerere",
            [],
            "Common Git config and rr-cache are shared; stale resolutions can silently drop content.",
            "Disable rerere for the entire rebase including continue; compare original and rebased git show --name-only commit file lists and audit unexpected drops. In zsh use while IFS= read -r. Provisioning does not rebase or alter Git config.",
        )
    count = sum(
        record.startswith("worktree ")
        for record in git(repo, "worktree", "list", "--porcelain").splitlines()
    )
    add(
        "worktree-budget",
        [],
        "Each checkout owns build outputs and consumes the disk budget.",
        f"Existing worktrees: {count}. Budget the estimate against free space; reuse an authorized lane for replacement work.",
    )
    targets = [
        (Path(p).parent / "target").as_posix()
        for p in paths
        if Path(p).name == "Cargo.toml"
        and Path(p).parent != Path(".")
        and not p.startswith("vendor/")
    ]
    if targets:
        add(
            "nested-target",
            sorted(targets),
            "Nested Rust target directories can consume about 4 GiB each and evade root-only cleanup.",
            f"Include {', '.join(sorted(targets))} in the budget and worktree-close inventory; never delete outputs of a live or unverified checkout.",
        )
    return sorted(result, key=lambda h: h["id"])


def atomic_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=".profile-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w") as output:
            json.dump(value, output, indent=2, sort_keys=True)
            output.write("\n")
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def reusable(
    cached: object, main: Path, digest: str, manifests: list, subs: list
) -> bool:
    """Reject malformed cache entries; cached policy is never an inferred default."""
    try:
        expected = set(
            "schema_version main_worktree manifest_hash manifests package_managers submodules toolchains entry_points policy recipe hazards".split()
        )
        if not isinstance(cached, dict) or set(cached) != expected:
            return False
        keys(
            cached,
            {
                "schema_version",
                "main_worktree",
                "manifest_hash",
                "manifests",
                "package_managers",
                "submodules",
                "toolchains",
                "entry_points",
                "policy",
                "recipe",
                "hazards",
            },
        )
        if (
            cached["schema_version"] != 1
            or cached["main_worktree"] != str(main)
            or cached["manifest_hash"] != digest
            or cached["manifests"] != manifests
            or cached["submodules"] != subs
        ):
            return False
        recipe = cached["recipe"]
        keys(
            recipe,
            {
                "prepare",
                "smoke",
                "unresolved",
                "shared_dependencies",
                "target_paths",
                "disk_estimate_gib",
                "disk_estimate_basis",
            },
        )
        commands(recipe["prepare"])
        commands(recipe["smoke"])
        for name in ("unresolved", "shared_dependencies", "target_paths"):
            if not isinstance(recipe[name], list) or any(
                not isinstance(p, str) for p in recipe[name]
            ):
                return False
        if not recipe["smoke"] and not recipe["unresolved"]:
            return False
        for name in ("shared_dependencies", "target_paths"):
            for path in recipe[name]:
                relative(path)
        if (
            type(recipe["disk_estimate_gib"]) not in (int, float)
            or not 0 < recipe["disk_estimate_gib"] < 100000
        ):
            return False
        if not isinstance(recipe["disk_estimate_basis"], str):
            return False
        for name in ("package_managers", "toolchains", "entry_points", "hazards"):
            if not isinstance(cached[name], list):
                return False
        for package in cached["package_managers"]:
            if (
                set(package) != {"path", "manager", "version", "lockfile", "workspaces"}
                or package["manager"] not in LOCKS
            ):
                return False
            relative(package["path"])
            if package["version"] is not None and not isinstance(
                package["version"], str
            ):
                return False
            if package["lockfile"] is not None:
                relative(package["lockfile"])
            if not isinstance(package["workspaces"], list) or any(
                not isinstance(p, str) for p in package["workspaces"]
            ):
                return False
        for entry in cached["entry_points"]:
            if set(entry) != {"kind", "path", "commands"} or entry["kind"] not in {
                "cargo",
                "package",
                "cmake",
                "make",
                "just",
                "run",
                "xcode",
                "gradle",
            }:
                return False
            relative(entry["path"])
            if not isinstance(entry["commands"], list) or any(
                not isinstance(c, str) for c in entry["commands"]
            ):
                return False
        for tool in cached["toolchains"]:
            if set(tool) != {"kind", "path", "pin"} or tool["kind"] not in {
                "rust",
                "node",
                "cmake",
                "xcode",
                "gradle",
                "android-ndk",
            }:
                return False
            relative(tool["path"])
            if tool["pin"] is not None and not isinstance(tool["pin"], str):
                return False
        return True
    except (KeyError, TypeError, ValueError):
        return False


def scan(repo: Path, *, use_cache: bool = True) -> dict:
    repo = Path(git(Path(repo).resolve(), "rev-parse", "--show-toplevel"))
    main = main_worktree(repo)
    subs = sorted(submodules(repo), key=lambda s: s["path"])
    paths = input_paths(repo, subs)
    manifests = [
        {"path": p, "sha256": hashlib.sha256((repo / p).read_bytes()).hexdigest()}
        for p in paths
    ]
    identity = {"manifests": manifests, "submodules": subs}
    digest = hashlib.sha256(
        json.dumps(identity, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    config = overrides(repo)
    policy = {
        "gate_command": config.get("gate_command"),
        "commit_style": config.get("commit_style"),
        "require_exact_head_gate": config.get("require_exact_head_gate", False),
        "submodule_update": config.get("submodules", {}).get("update", "recursive"),
    }
    cache = main / "temp/.bus/repo-profile.json"
    cached = None
    if use_cache:
        try:
            cached = json.loads(cache.read_text())
        except (OSError, ValueError):
            pass
    if (
        use_cache
        and reusable(cached, main, digest, manifests, subs)
        and cached["policy"] == policy
    ):
        current_hazards = hazards(repo, paths, subs, cached["toolchains"])
        if current_hazards != cached["hazards"]:
            cached["hazards"] = current_hazards
            atomic_json(cache, cached)
        return cached
    packages = package_info(repo, paths)
    tools, entries = inventory(repo, paths)
    result = {
        "schema_version": 1,
        "main_worktree": str(main),
        "manifest_hash": digest,
        "manifests": manifests,
        "package_managers": packages,
        "submodules": subs,
        "toolchains": tools,
        "entry_points": entries,
        "policy": policy,
        "recipe": infer_recipe(repo, paths, packages, config),
        "hazards": hazards(repo, paths, subs, tools),
    }
    if use_cache:
        atomic_json(cache, result)
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["scan"])
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument(
        "--no-cache",
        action="store_true",
        help="fresh JSON without cache access or directory creation",
    )
    args = parser.parse_args()
    try:
        result = scan(args.repo, use_cache=not args.no_cache)
        print(json.dumps(result, indent=2, sort_keys=True))
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"repo-profile: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
