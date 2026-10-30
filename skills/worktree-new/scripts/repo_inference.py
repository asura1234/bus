"""Infer build ownership, finite recipes and generic local hazards."""

import fnmatch
from functools import cache
import json
from pathlib import Path
import re
import subprocess
import tomllib

from repo_support import git, command


def workspace_glob_matches(path, pattern):
    segments = path.split("/")
    globs = pattern.rstrip("/").split("/")

    @cache
    def matches(segment, glob):
        if glob == len(globs):
            return segment == len(segments)
        if globs[glob] == "**":
            return matches(segment, glob + 1) or (
                segment < len(segments) and matches(segment + 1, glob)
            )
        return (
            segment < len(segments)
            and fnmatch.fnmatchcase(segments[segment], globs[glob])
            and matches(segment + 1, glob + 1)
        )

    return matches(0, 0)


def cargo_roots(repo, paths):
    manifests = {
        p: tomllib.loads((repo / p).read_text())
        for p in paths
        if Path(p).name == "Cargo.toml"
    }
    dependencies = set()

    def references(value, parent):
        if isinstance(value, dict):
            if isinstance(value.get("path"), str):
                candidate = (repo / parent / value["path"] / "Cargo.toml").resolve()
                if candidate.is_relative_to(repo):
                    dependencies.add(candidate.relative_to(repo).as_posix())
            for child in value.values():
                references(child, parent)
        elif isinstance(value, list):
            for child in value:
                references(child, parent)

    for name, data in manifests.items():
        references(data, Path(name).parent)
    roots = []
    for name, data in manifests.items():
        if name != "Cargo.toml" and name in dependencies:
            continue
        parent = Path(name).parent
        owned = False
        if "workspace" not in data:
            for other, owner in manifests.items():
                base = Path(other).parent
                if (
                    other == name
                    or "workspace" not in owner
                    or base not in parent.parents
                ):
                    continue
                workspace = owner["workspace"]
                relative = parent.relative_to(base).as_posix()
                if any(
                    workspace_glob_matches(relative, pattern)
                    for pattern in workspace.get("exclude", [])
                ):
                    continue
                if any(
                    workspace_glob_matches(relative, pattern)
                    for pattern in workspace.get("members", [])
                ):
                    owned = True
                    break
        if not owned:
            roots.append(name)
    return sorted(roots)


def node_workspace_owner(packages, path):
    target = Path(path)
    owners = []
    for package in packages:
        base = Path(package["path"]).parent
        if base == target or not target.is_relative_to(base):
            continue
        relative = target.relative_to(base).as_posix()
        patterns = package["workspaces"]
        if any(
            workspace_glob_matches(relative, p)
            for p in patterns
            if not p.startswith("!")
        ) and not any(
            workspace_glob_matches(relative, p[1:])
            for p in patterns
            if p.startswith("!")
        ):
            owners.append(package)
    return max(owners, key=lambda p: len(Path(p["path"]).parts), default=None)


def governing_node_pin(tools, cwd):
    directory = Path(cwd)
    candidates = [
        t
        for t in tools
        if t["kind"] == "node" and directory.is_relative_to(Path(t["path"]).parent)
    ]
    return max(
        candidates,
        key=lambda t: (
            len(Path(t["path"]).parts),
            Path(t["path"]).name == ".node-version",
        ),
        default=None,
    )


def numeric_node_pin(pin):
    return (
        isinstance(pin, str)
        and re.fullmatch(r"[0-9]+(?:\.[0-9]+){0,2}", pin) is not None
    )


def visible_directory_links(repo, private_paths):
    roots = [repo]
    for child in repo.iterdir():
        if (
            child.name
            not in {
                ".git",
                "node_modules",
                "target",
                "temp",
                "dist",
                ".next",
                "Pods",
                ".gradle",
            }
            and child.is_dir()
            and not child.is_symlink()
        ):
            roots.append(child)
    found = {
        p.relative_to(repo).as_posix()
        for root in roots
        for p in root.iterdir()
        if p.name != "node_modules" and p.is_symlink() and p.is_dir()
    }
    for name in private_paths:
        for parent in (repo / name, *(repo / name).parents):
            if parent == repo:
                break
            if parent.is_symlink():
                found.add(parent.relative_to(repo).as_posix())
    return sorted(found)


def infer_recipe(
    repo: Path,
    paths: list[str],
    packages: list[dict],
    config: dict,
    tools=None,
) -> dict:
    prepare, smoke, unresolved = [], [], []
    cargo = cargo_roots(repo, paths)
    for name in cargo:
        cwd = Path(name).parent.as_posix()
        manifest = tomllib.loads((repo / name).read_text())
        workspace = manifest.get("workspace", {})
        # Without default-members, a package workspace selects only its root.
        workspace_scope = (
            ["--workspace"]
            if "package" in manifest
            and "workspace" in manifest
            and "default-members" not in workspace
            else []
        )
        if (Path(cwd) / "Cargo.lock").as_posix() not in paths:
            unresolved.append(
                f"{name}: locked Cargo dependency resolution requires Cargo.lock"
            )
        prepare.append(command(["cargo", "fetch", "--locked"], cwd))
        smoke.extend(
            [
                command(["cargo", "build", "--locked", *workspace_scope], cwd),
                command(["cargo", "test", "--locked", *workspace_scope], cwd),
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
                    r"\b(watch\w*|electron|run-ios|run-android|detox)\b(?!\s*=\s*false)",
                    scripts[s],
                    re.I,
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
    for package in packages:
        if (
            package["path"] != "package.json"
            and node_workspace_owner(packages, Path(package["path"]).parent) is None
        ):
            unresolved.append(
                f"{package['path']}: nested Node product needs an explicit headless worktree.smoke recipe"
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
    for item in prepare + smoke:
        pin = governing_node_pin(tools or [], item["cwd"])
        if pin and not numeric_node_pin(pin["pin"]):
            unresolved.append(
                f"{pin['path']}: resolve the Node alias to a numeric pin before provisioning"
            )
    targets = [(Path(p).parent / "target").as_posix() for p in cargo]
    nested_count = sum(p != "target" for p in targets)
    return {
        "prepare": prepare,
        "smoke": smoke,
        "unresolved": unresolved,
        "shared_dependencies": wt.get("shared_dependencies", []),
        "private_paths": wt.get("private_paths", []),
        "target_paths": targets,
        "disk_estimate_gib": wt.get("disk_estimate_gib", 15 + 4 * nested_count),
        "disk_estimate_basis": "15 GiB planning allowance; native Rust targets add 4 GiB each",
    }


def hazards(
    repo: Path, paths: list[str], subs: list[dict], tools: list[dict], recipe: dict
) -> list[dict]:
    result = []

    def add(identity, affected, reason, action):
        result.append(
            {"id": identity, "paths": affected, "reason": reason, "action": action}
        )

    links = visible_directory_links(repo, recipe["private_paths"])
    if links:
        add(
            "shared-deps",
            links,
            "Directory links can expose shared mutable dependency state to another checkout.",
            "Keep configured private_paths local before changing dependency locks; immutable cache children may remain linked.",
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
    targets = [p for p in recipe["target_paths"] if p != "target"]
    if targets:
        add(
            "nested-target",
            sorted(targets),
            "Nested Rust target directories can consume about 4 GiB each and evade root-only cleanup.",
            f"Include {', '.join(sorted(targets))} in the budget and worktree-close inventory; never delete outputs of a live or unverified checkout.",
        )
    return sorted(result, key=lambda h: h["id"])
