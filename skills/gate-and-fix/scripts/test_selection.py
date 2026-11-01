"""Conservative affected-test selection for the Bus crate and tooling."""
from __future__ import annotations

import subprocess
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

PYTHON = ("scripts", "skills", "cli_extensions", "tools", "packaging")
SHARED_SKILLS = {"_bus", "gate-and-fix", "worktree-new", "rebase-origin-main"}
INTEGRATION = {
    "api": ("api",), "client": ("client", "room-screen"),
    "server": ("server",), "cli": ("cli",), "tui": ("tui-driver",),
}
RUST_OWNERS = {"agents", "cli", "client", "messaging", "platform", "protocol",
               "server", "terminal", "utils", "devtools"}


@dataclass(frozen=True)
class Selection:
    full: bool
    rust_filter: str | None
    python: tuple[str, ...]
    dev_tools: bool
    reason: str


def select(paths: list[str], *, full: bool = False) -> Selection:
    rust, python = set(), set()
    dev_tools = False
    for name in sorted(set(paths)):
        path = PurePosixPath(name)
        if not name or path.is_absolute() or any(p in {"", ".", ".."} for p in name.split("/")):
            raise ValueError("paths must be normalized repository-relative paths")
        parts = path.parts
        if parts[0] == "src" and path.suffix == ".rs" and (
            "tests" in parts[2:-1] or path.name.endswith("_test.rs")
        ) and len(parts) > 2 and parts[1] in RUST_OWNERS:
            owner = parts[1]
            if owner == "devtools":
                dev_tools = True
            else:
                rust.add(f"(binary(bus) & test(/^{owner}::/))")
        elif len(parts) > 2 and parts[0] == "tests" and parts[1] in INTEGRATION:
            if parts[1] == "tui":
                dev_tools = True
            else:
                rust.update(f"binary({target})" for target in INTEGRATION[parts[1]])
                dev_tools |= parts[1] == "cli"
        elif parts[0] == "skills" and len(parts) > 2 and path.suffix == ".py" and parts[1] not in SHARED_SKILLS:
            python.add("/".join(parts[:2]))
        elif parts[0] in {"tools", "scripts", "cli_extensions", "packaging"} and path.suffix == ".py":
            python.add(parts[0])
        elif path.suffix == ".md":
            # Documentation has Python contract tests, including installed skill bundles.
            python.update(PYTHON)
        else:
            full = True
    if full:
        return Selection(True, None, PYTHON, True, "explicit full mode or shared/unknown input")
    return Selection(False, " | ".join(sorted(rust)) or "none()", tuple(sorted(python)),
                     dev_tools, "explicit test/tooling owners; coverage floor skipped")


def changed_paths(root: Path, base: str) -> list[str]:
    def git(*args: str) -> bytes:
        result = subprocess.run(["git", *args], cwd=root, capture_output=True)
        if result.returncode:
            raise ValueError(result.stderr.decode(errors="replace").strip())
        return result.stdout
    # --no-renames reports both old and new names. NUL delimiters preserve spaces/newlines.
    ancestor = git("merge-base", base, "HEAD").decode().strip()
    streams = [git("diff", "--name-only", "-z", "--no-renames", ancestor),
               git("ls-files", "--others", "--exclude-standard", "-z")]
    return sorted({path.decode(errors="surrogateescape") for stream in streams
                   for path in stream.split(b"\0") if path})
