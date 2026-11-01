"""Conservative affected-suite selection for the Bus crate and tooling."""
from __future__ import annotations

import subprocess
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

from suite_references import References

PYTHON = ("scripts", "skills", "cli_extensions", "tools", "packaging")
SHARED_SKILLS = {"_bus", "gate-and-fix", "worktree-new", "rebase-origin-main"}
RUST_OWNERS = {"agents", "cli", "client", "messaging", "platform", "protocol",
               "server", "terminal", "utils", "devtools"}


@dataclass(frozen=True)
class Selection:
    full: bool
    rust_filter: str | None
    python: tuple[str, ...]
    dev_tools: bool
    reason: str
    coverage_floor: bool = False
    binary: bool = False


def select(paths: list[str], *, full: bool = False, root: Path | None = None) -> Selection:
    coverage_floor = full
    rust, python = set(), set()
    dev_tools = False
    root = root or Path(__file__).resolve().parents[3]
    for name in paths:
        if not name or PurePosixPath(name).is_absolute() or any(p in {"", ".", ".."} for p in name.split("/")):
            raise ValueError("paths must be normalized repository-relative paths")
    def can_narrow(name):
        path = PurePosixPath(name)
        parts = path.parts
        if path.suffix == '.py':
            return parts[0] in PYTHON and not (parts[0] == 'skills' and len(parts) > 1 and parts[1] in SHARED_SKILLS)
        return path.suffix == '.rs' and (parts[0] == 'tests' or (
            len(parts) > 2 and parts[0] == 'src' and parts[1] in RUST_OWNERS and
            ('tests' in parts[2:-1] or path.name.endswith('_test.rs'))))
    if full or any(not can_narrow(name) for name in paths):
        return Selection(True, None, PYTHON, True, "explicit full mode or shared/unknown input", coverage_floor, True)
    references = References(root) if paths and not full else None
    full |= bool(references and references.uncertain)
    for name in sorted(set(paths)):
        path = PurePosixPath(name)
        if not name or path.is_absolute() or any(p in {"", ".", ".."} for p in name.split("/")):
            raise ValueError("paths must be normalized repository-relative paths")
        parts = path.parts
        if full:
            break
        if name not in references.paths or references.cross_owner(name):
            full = True
        elif parts[0] == "src" and path.suffix == ".rs" and (
            "tests" in parts[2:-1] or path.name.endswith("_test.rs")
        ) and len(parts) > 2 and parts[1] in RUST_OWNERS and references.incoming(name):
            owner = parts[1]
            if owner == "devtools":
                dev_tools = True
            else:
                rust.add(f"(binary(bus) & test(/^{owner}::/))")
        elif parts[0] == "tests" and path.suffix == ".rs":
            targets = references.integration_targets(name)
            if not targets:
                full = True
            rust.update(f"binary({target})" for target in targets if target != "tui-driver")
            dev_tools |= bool(targets.intersection({"cli", "tui-driver"}))
        elif parts[0] == "skills" and len(parts) > 2 and path.suffix == ".py" and parts[1] not in SHARED_SKILLS:
            python.add("/".join(parts[:2]))
        elif parts[0] in {"tools", "scripts", "cli_extensions", "packaging"} and path.suffix == ".py":
            python.add(parts[0])
        else:
            full = True
    if full:
        return Selection(True, None, PYTHON, True, "explicit full mode or cross-owner/shared/unknown input", coverage_floor, True)
    tests = [name for name in references.sources if name.endswith('_test.py') and any(
        name.startswith(prefix + '/') for prefix in python
    )] if references else []
    return Selection(False, " | ".join(sorted(rust)) or "none()", tuple(sorted(python)),
                     dev_tools, "source-reference owners; coverage floor skipped", binary=bool(references and references.needs_binary(tests)))


def changed_paths(root: Path, base: str) -> list[str]:
    def git(*args: str) -> bytes:
        result = subprocess.run(["git", *args], cwd=root, capture_output=True)
        if result.returncode:
            raise ValueError(result.stderr.decode(errors="replace").strip())
        return result.stdout
    # --no-renames reports both old and new names. NUL delimiters preserve spaces/newlines.
    ancestor = git("merge-base", base, "HEAD").decode().strip()
    streams = [git("diff", "--name-only", "-z", "--no-renames", ancestor, "HEAD"),
               git("diff", "--cached", "--name-only", "-z", "--no-renames"),
               git("diff", "--name-only", "-z", "--no-renames"),
               git("ls-files", "--others", "--exclude-standard", "-z")]
    return sorted({path.decode(errors="surrogateescape") for stream in streams
                   for path in stream.split(b"\0") if path})
