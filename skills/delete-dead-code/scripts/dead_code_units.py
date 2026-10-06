"""Scope layer of delete-dead-code: unit derivation, unit state, and batch planning (CLI entry in dead_code_scope.py)."""

from __future__ import annotations

import subprocess
from dataclasses import dataclass, field
from pathlib import Path


DEFAULT_MAX_PARALLEL = 5
"""Per-round concurrency cap. When there are more modules than this, rounds run serially instead of all at once."""

MODULE_MARKER = "AGENTS.md"
"""Module root marker: the nearest ancestor directory that carries an `AGENTS.md`."""

REPOSITORY_MODULE = "<repository-root>"
"""Ownership identity for loose top-level files (`Cargo.toml`, `justfile`, ...). By definition it owns no directory.

Top-level subtrees without an `AGENTS.md` (`src/`, `scripts/`, `tests/`) do **not** belong here: each one
becomes its own unit. Folding them into one "repository root" bucket has two consequences, and the second
dogfood round hit both: touching one file in `scripts/` would hand the agent `src/`, `.github/`, and `tests/`
as well (exactly the out-of-scope shape this skill exists to prevent); and judging membership by "the path has
no `/`" would flag `scripts/x.py` as out of scope.
"""


class DeadCodeScopeError(Exception):
    """The scope cannot be derived trustworthily, or an artifact cannot be verified."""


@dataclass(frozen=True)
class Module:
    name: str
    file_count: int
    excludes: tuple[str, ...] = field(default=())
    directories: tuple[str, ...] = field(default=())

    def as_json(self) -> dict[str, object]:
        # Agent briefs must quote excludes and directories verbatim instead of re-deriving them from the tree.
        return {
            "module": self.name,
            "fileCount": self.file_count,
            "excludes": list(self.excludes),
            "directories": list(self.directories),
        }


def _git(repository: Path, *arguments: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(repository), *arguments],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise DeadCodeScopeError(f"git {' '.join(arguments)} failed ({result.returncode}): {result.stderr.strip()}")
    return result.stdout


def changed_paths(repository: Path, base: str) -> list[str]:
    """Files in the PR's three-dot diff against base."""
    output = _git(repository, "diff", "--name-only", f"{base}...HEAD")
    return sorted({line for line in output.splitlines() if line})


def tracked_paths(repository: Path, directories: list[str]) -> list[str]:
    """Every tracked file under the given directories.

    A missing directory fails closed: silently scanning nothing because a directory name was misspelled is
    far worse than an error. It returns in the shape of "this area is clean", and nobody can tell from the
    output that the scan never happened.
    """
    if not directories:
        raise DeadCodeScopeError("--directories must not be empty")
    paths: set[str] = set()
    for directory in directories:
        normalized = Path(directory).as_posix().rstrip("/")
        if not (repository / normalized).is_dir():
            raise DeadCodeScopeError(f"directory does not exist: {directory}")
        entries = [line for line in _git(repository, "ls-files", "-z", "--", normalized).split("\0") if line]
        if not entries:
            raise DeadCodeScopeError(f"directory has no tracked files: {directory}")
        paths.update(entries)
    return sorted(paths)


def module_of(repository: Path, path: str) -> str:
    """The module root a path belongs to: the nearest ancestor directory that contains `AGENTS.md`.

    The repository root is never returned as `"."`, even when it has an `AGENTS.md`: that name matches no real
    path prefix and reads like "the whole repository is in scope". When no module marker is found it falls back
    to the **top-level subtree** (`src/client/x.rs` -> `src`); only loose top-level files belong to
    `REPOSITORY_MODULE`, which `_in_scope` judges separately.
    """
    current = Path(path).parent
    while current != Path("."):
        if (repository / current / MODULE_MARKER).is_file():
            return current.as_posix()
        current = current.parent
    parts = Path(path).parts
    return parts[0] if len(parts) > 1 else REPOSITORY_MODULE


def nested_modules(repository: Path, name: str) -> tuple[str, ...]:
    """Subdirectories under `name` that carry their own `AGENTS.md`: they are someone else's modules, not `name`'s.

    This list must come from the script: letting every agent derive "what does my module actually contain" from
    the tree copies one mechanical fact into a dozen briefs, and one bad copy is an out-of-bounds deletion.

    Markers come from `git ls-files`, not a filesystem walk: third-party packages under `target/` or
    `node_modules/` may ship their own `AGENTS.md`, and a walk would report them as nested modules.
    """
    if name == REPOSITORY_MODULE:
        return ()
    if not (repository / name).is_dir():
        return ()
    nested = {
        Path(entry).parent.as_posix()
        for entry in _git(repository, "ls-files", "-z", "--", name).split("\0")
        if entry and Path(entry).name == MODULE_MARKER and Path(entry).parent.as_posix() != name
    }
    return tuple(sorted(nested))


def _directories_of(entries: list[str], root: str) -> tuple[str, ...]:
    """The set of directories covered by `entries` (including `root` itself)."""
    directories = {root}
    for entry in entries:
        parent = Path(entry).parent.as_posix()
        while parent not in (root, ".", ""):
            directories.add(parent)
            parent = Path(parent).parent.as_posix()
    return tuple(sorted(directories))


def module_directories(repository: Path, name: str, excludes: tuple[str, ...] = ()) -> tuple[str, ...]:
    """Directories the module owns: its root plus every subdirectory with tracked files, minus nested modules.

    Directories are derived from `git ls-files`, not a filesystem walk: `node_modules/`, `target/`, `out/`, and
    `dist/` are untracked, and a walk would write them into the brief, handing the agent a map that is mostly
    build output. This is the same reason guide.md says "use `git grep`, not `grep -r`"; there the consequence
    is a missed deletion, here it is a scan budget burned on stale artifacts.

    `REPOSITORY_MODULE` has no directories: by definition it owns only loose top-level files, and giving it a
    directory list would read like "the whole repository".
    """
    if name == REPOSITORY_MODULE:
        return ()
    excluded_prefixes = tuple(f"{item}/" for item in excludes)
    entries = [
        entry
        for entry in _git(repository, "ls-files", "-z", "--", name).split("\0")
        if entry and not entry.startswith(excluded_prefixes)
    ]
    return _directories_of(entries, name)


def _requested_directory(repository: Path, directory: str) -> str:
    """Canonical form of an explicit directory: a repository-relative POSIX path. Relative, absolute, and `./`
    spellings of the same directory must yield the same name, otherwise the overlap check can be bypassed by
    spelling. The repository root cannot be a unit: it contains everything and has no name to prefix-match.
    """
    root = repository.resolve()
    candidate = Path(directory)
    absolute = (candidate if candidate.is_absolute() else root / candidate).resolve()
    try:
        relative = absolute.relative_to(root)
    except ValueError:
        raise DeadCodeScopeError(f"directory is outside the repository: {directory}") from None
    if relative == Path("."):
        raise DeadCodeScopeError(
            "an explicit directory cannot be the repository root: the whole repository is not a unit; "
            "pass top-level subdirectories instead"
        )
    return relative.as_posix()


def scope(repository: Path, base: str | None = None, directories: list[str] | None = None) -> list[Module]:
    """Modules in scope, sorted by file count descending, then by name ascending.

    Without `directories` the scope is the modules this PR touches (`base...HEAD`); with `directories` it is the
    modules owning every tracked file under those directories. Both fail closed: no agent should be dispatched
    for an empty scope.
    """
    if directories:
        # In explicit-directory mode **each requested directory is one unit**; nested modules are not split out.
        # The reason is "can it act", not "how large is the scan": the canonical home of real dead code is
        # usually a sibling module (base64 in tools wanted to move to codecs, the Windows helper to process,
        # canSplitClip in core to converge into model). An agent owning only one leaf module can only record
        # HANDOFF for all of these; an agent owning the whole tree just does them. Granularity is therefore
        # the caller's choice: pass more subdirectories to slice finer.
        requested = [_requested_directory(repository, directory) for directory in directories]
        # Pairwise-disjoint units are the premise of sharing one worktree: if a parent and child directory are
        # both units, one file has two owners.
        for index, outer in enumerate(requested):
            for inner in requested[index + 1 :]:
                if outer == inner or inner.startswith(f"{outer}/") or outer.startswith(f"{inner}/"):
                    raise DeadCodeScopeError(
                        f"explicit directories overlap: {outer} and {inner}; every path may have only one owner, "
                        "pass directories that do not contain each other"
                    )
        units: list[Module] = []
        for normalized in requested:
            entries = tracked_paths(repository, [normalized])
            units.append(
                Module(
                    name=normalized,
                    file_count=len(entries),
                    excludes=(),
                    directories=_directories_of(entries, normalized),
                )
            )
        return sorted(units, key=lambda unit: (-unit.file_count, unit.name))
    else:
        if base is None:
            raise DeadCodeScopeError("one of --base or --directories is required")
        paths = changed_paths(repository, base)
        if not paths:
            raise DeadCodeScopeError(f"no changed files relative to {base}; the scope is empty")
    counts: dict[str, int] = {}
    for path in paths:
        name = module_of(repository, path)
        counts[name] = counts.get(name, 0) + 1
    modules: list[Module] = []
    for name, count in sorted(counts.items(), key=lambda item: (-item[1], item[0])):
        excludes = nested_modules(repository, name)
        modules.append(
            Module(
                name=name,
                file_count=count,
                excludes=excludes,
                directories=module_directories(repository, name, excludes),
            )
        )
    return modules


def select_units(modules: list[Module], names: list[str] | None) -> list[Module]:
    """The units this invocation (or this artifact) owns; without names, every unit in scope.

    Names must match a unit in scope exactly: silently falling back to "every unit" on a misspelled unit name
    would widen a unit artifact's boundary check to the whole scope, and read exactly like a pass.
    """
    if not names:
        return modules
    by_name = {module.name: module for module in modules}
    selected = list(dict.fromkeys(Path(name).as_posix().rstrip("/") for name in names))
    unknown = [name for name in selected if name not in by_name]
    if unknown:
        raise DeadCodeScopeError(
            f"--unit is not in this scope: {', '.join(unknown)}; units in scope: {', '.join(sorted(by_name))}"
        )
    return [by_name[name] for name in selected]


def unit_pathspecs(unit: Module) -> list[str]:
    """The git pathspecs a unit owns: the unit root, minus nested module subtrees (those are other units).

    Must be used **per unit**: `:(exclude)` applies to every positive pathspec in the same command, so putting a
    parent unit and its nested child unit in one command lets the parent's exclude swallow the child entirely.
    """
    if unit.name == REPOSITORY_MODULE:
        # Under glob magic `*` does not cross `/`: it matches only loose top-level files, matching the definition
        # of REPOSITORY_MODULE.
        return [":(glob)*"]
    return [unit.name, *(f":(exclude){item}" for item in unit.excludes)]


def dirty_paths(repository: Path, units: list[Module]) -> list[str]:
    """Uncommitted changes (including untracked files) inside this invocation's units; nothing outside them.

    Units are pairwise disjoint, so changes outside the units can only belong to other units. When parallel
    invocations share one worktree that is the normal state, not a reason to stop. `--ignore-submodules=all`
    excludes submodule gitlinks: this skill never edits submodules, and a submodule's own HEAD and worktree
    state are unrelated to the units.
    """
    paths: set[str] = set()
    for unit in units:
        entries = _git(
            repository,
            "status",
            "--porcelain",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=all",
            "--",
            *unit_pathspecs(unit),
        ).split("\0")
        index = 0
        while index < len(entries):
            entry = entries[index]
            index += 1
            if not entry:
                continue
            paths.add(entry[3:])
            if "R" in entry[:2] or "C" in entry[:2]:
                # Under `-z` a rename/copy record is followed by the original path; the pathspec already
                # guarantees it falls in this unit too.
                index += 1
    return sorted(paths)


def preflight(
    repository: Path,
    base: str | None,
    directories: list[str] | None,
    unit_names: list[str] | None = None,
) -> list[str]:
    """Conditions that must hold before each track starts; an empty list means it may start."""
    problems: list[str] = []
    branch = _git(repository, "branch", "--show-current").strip()
    if not branch or branch in ("main", "master"):
        problems.append(f"delete-dead-code runs only on a feature branch (current: {branch or '<detached>'})")
    units = select_units(scope(repository, base, directories), unit_names)
    for path in dirty_paths(repository, units):
        problems.append(f"{path}: uncommitted change inside a unit this invocation owns")
    return problems


def plan_batches(modules: list[Module], max_parallel: int) -> list[list[Module]]:
    """Spread modules over the fewest rounds while keeping each round's scan load as even as possible.

    Rounds are serial and a round takes as long as its **largest** module, so modules are dealt round-robin in
    descending file-count order (instead of sliced sequentially): large modules land in different rounds, so no
    round holds two giant modules while another holds only small ones.
    """
    if max_parallel < 1:
        raise DeadCodeScopeError(f"--max-parallel must be >= 1, got {max_parallel}")
    if not modules:
        raise DeadCodeScopeError("no modules to plan")
    round_count = -(-len(modules) // max_parallel)
    batches: list[list[Module]] = [[] for _ in range(round_count)]
    for index, module in enumerate(modules):
        batches[index % round_count].append(module)
    return batches


def _owns(module: Module, path: str) -> bool:
    """Ownership with the same semantics as `unit_pathspecs`: under the unit root and under no nested module."""
    if module.name == REPOSITORY_MODULE:
        # `REPOSITORY_MODULE` owns only loose top-level files, so "the path has no `/`" is exact here, not an
        # approximation.
        return "/" not in path
    if not (path == module.name or path.startswith(f"{module.name}/")):
        return False
    return not any(path == item or path.startswith(f"{item}/") for item in module.excludes)


def _in_scope(path: str, modules: list[Module]) -> bool:
    return any(_owns(module, path) for module in modules)
