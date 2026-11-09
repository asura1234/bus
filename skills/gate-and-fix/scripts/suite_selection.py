"""Conservative affected-suite selection for the Bus crate and tooling."""
from __future__ import annotations

import re
import subprocess
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

from suite_references import References, owner
from suite_lanes import PYTHON, SHARED_SKILLS, classify, consumers, kind, rust_test

RUST_OWNERS = {"agents", "cli", "client", "messaging", "platform", "protocol",
               "server", "terminal", "utils", "devtools"}
_SERDE = re.compile(r"#\[serde\b|\bserde::|\b(?:Serialize|Deserialize)\b")
_TEMPLATE = re.compile(r"""include_str!\s*\(\s*"[^"]+\.(?:html|json)" """)


def production_rust(name: str) -> bool:
    path = PurePosixPath(name)
    parts = path.parts
    return (path.suffix == ".rs" and len(parts) > 2 and parts[0] == "src"
            and parts[1] in RUST_OWNERS and "tests" not in parts[2:-1]
            and not path.name.endswith("_test.rs"))


def integration_foreign_only(references: References, name: str) -> bool:
    """An integration crate that includes one production file is selected, not broadened."""
    if not production_rust(name):
        return False
    foreign = [source for source in references.incoming(name) if owner(source) != owner(name)]
    return bool(foreign) and all(
        PurePosixPath(source).parts[:1] == ("tests",) and PurePosixPath(source).suffix == ".rs"
        for source in foreign)


def blind_surface(name: str, source: str, mask) -> bool:
    """Serde shapes, CLI surfaces, and TUI strings have no module edge to their tests."""
    if (name == "src/main.rs" or "/model/" in f"/{name}"
            or name.startswith(("src/cli/", "src/terminal/", "src/protocol/"))):
        return True
    code = mask(source) if source else ""
    return bool(_SERDE.search(code) or _TEMPLATE.search(source))


@dataclass(frozen=True)
class Selection:
    full: bool
    rust_filter: str | None
    python: tuple[str, ...]
    dev_tools: bool
    reason: str
    coverage_floor: bool = False
    binary: bool = False
    python_reason: str = ""
    rust_reason: str = ""
    doc_tests: tuple[str, ...] = ()


def select(paths: list[str], *, full: bool = False, root: Path | None = None) -> Selection:
    coverage_floor = full
    rust, python = set(), set()
    dev_tools = False
    root = root or Path(__file__).resolve().parents[3]
    for name in paths:
        if not name or PurePosixPath(name).is_absolute() or any(p in {"", ".", ".."} for p in name.split("/")):
            raise ValueError("paths must be normalized repository-relative paths")
    if any(kind(name) == "shared" or not (root / name).is_file() for name in paths):
        return Selection(True, None, PYTHON, True, "shared/unknown input", coverage_floor,
                         True, "shared/unknown input", "shared/unknown input")
    references = References(root)
    lanes = classify(paths, references)

    def finish(broad, rust_filter, python, dev_tools, reason):
        python = tuple(python) if lanes.python else lanes.dependent_python
        tests = [name for name in references.sources if name.endswith('_test.py') and any(
            name == prefix or name.startswith(prefix + '/') for prefix in python
        )]
        docs = tuple(name for name in lanes.doc_tests if name not in tests)
        return Selection(broad, rust_filter if lanes.rust else "none()", python,
                         dev_tools and lanes.rust, reason, coverage_floor and lanes.rust,
                         references.needs_binary([*tests, *docs]),
                         lanes.python_reason, lanes.rust_reason, docs)
    def can_narrow(name):
        path = PurePosixPath(name)
        parts = path.parts
        if path.suffix == '.py':
            return parts[0] in PYTHON and not (parts[0] == 'skills' and len(parts) > 1 and parts[1] in SHARED_SKILLS)
        return path.suffix == '.rs' and (parts[0] == 'tests' or (
            len(parts) > 2 and parts[0] == 'src' and parts[1] in RUST_OWNERS))
    if full or any(not can_narrow(name) for name in paths):
        return finish(True, None, PYTHON, True, "explicit full mode or shared/unknown input")
    full |= bool(references and references.uncertain)
    for name in sorted(set(paths)):
        path = PurePosixPath(name)
        if not name or path.is_absolute() or any(p in {"", ".", ".."} for p in name.split("/")):
            raise ValueError("paths must be normalized repository-relative paths")
        parts = path.parts
        if full:
            break
        if name not in references.paths or (
            references.cross_owner(name) and not integration_foreign_only(references, name)
        ):
            full = True
        elif parts[0] == "src" and path.suffix == ".rs" and (
            "tests" in parts[2:-1] or path.name.endswith("_test.rs")
        ) and len(parts) > 2 and parts[1] in RUST_OWNERS and references.incoming(name):
            # Integration crates can include a source container and its cfg(test)
            # children, whose behavior also depends on that separate crate context.
            if references.integration_targets(name):
                full = True
                break
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
        elif production_rust(name):
            if blind_surface(name, references.sources.get(name, ""), references.mask_rust):
                full = True
                break
            owner_name = parts[1]
            if owner_name == "devtools":
                dev_tools = True
            else:
                rust.add(f"(binary(bus) & test(/^{owner_name}::/))")
            targets = references.integration_targets(name)
            if integration_foreign_only(references, name) and not targets:
                full = True
                break
            rust.update(f"binary({target})" for target in targets if target != "tui-driver")
            dev_tools |= bool(targets.intersection({"cli", "tui-driver"}))
            for consumer in consumers(references, [name]):
                if not rust_test(consumer):
                    continue
                cparts = PurePosixPath(consumer).parts
                if cparts[0] == "tests":
                    found = references.integration_targets(consumer)
                    if not found:
                        full = True
                        break
                    rust.update(f"binary({target})" for target in found if target != "tui-driver")
                    dev_tools |= bool(found.intersection({"cli", "tui-driver"}))
                elif len(cparts) > 2 and cparts[0] == "src" and cparts[1] in RUST_OWNERS:
                    # A source test compiled into another crate is not this component's unit filter.
                    if references.integration_targets(consumer):
                        full = True
                        break
                    if cparts[1] == "devtools":
                        dev_tools = True
                    else:
                        rust.add(f"(binary(bus) & test(/^{cparts[1]}::/))")
                else:
                    full = True
                    break
        else:
            full = True
    if full:
        return finish(True, None, PYTHON, True, "cross-owner/shared/unknown input")
    return finish(False, " | ".join(sorted(rust)) or "none()", sorted(python),
                  dev_tools, "source-reference owners; coverage floor skipped")


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
