"""Report crate-qualified Rust dependencies against architecture graph 3a.

This is a lexical check, including cfg-gated/test code; it does not resolve
aliases, super/self paths or macro expansion. Legacy owners are approximate
until split modules move. The composition shim itself is not a component.
"""

from __future__ import annotations

import argparse
import re
from dataclasses import dataclass
from pathlib import Path

from tools.quality.hot_path import PROJECT_ROOT, mask_comments_and_literals

# Same-component and utils imports are implicit for every component.
DEPENDENCIES = {
    "utils": frozenset(),
    "platform": frozenset(),
    "protocol": frozenset({"platform"}),
    "agents": frozenset({"platform"}),
    "terminal": frozenset({"agents", "protocol", "platform"}),
    "messaging": frozenset({"agents", "protocol", "platform"}),
    "server": frozenset({"terminal", "messaging", "agents", "protocol", "platform"}),
    "client": frozenset({"messaging", "agents", "protocol", "platform"}),
    "cli": frozenset({"client", "messaging", "protocol", "platform"}),
}
LEGACY_ROOTS = {
    "agent_resume": "agents",
    "api": "protocol",
    "app": "server",
    "build_info": "utils",
    "bus": "messaging",
    "config": "utils",
    "copy_mode": "utils",
    "detect": "agents",
    "events": "terminal",
    "ghostty": "terminal",
    "home_path": "utils",
    "input": "protocol",
    "ipc": "platform",
    "kitty_graphics": "protocol",
    "layout": "server",
    "logging": "utils",
    "noninteractive_process": "platform",
    "pane": "terminal",
    "pane_state": "server",
    "persist": "server",
    "pty": "terminal",
    "raw_input": "protocol",
    "render_prof": "utils",
    "render_signal": "utils",
    "selection": "utils",
    "session": "utils",
    "sound": "platform",
    "terminal_effects": "client",
    "terminal_modes": "client",
    "terminal_notify": "client",
    "terminal_theme": "utils",
    "ui": "server",
    "workspace": "server",
}
# Longest-prefix overrides for split legacy modules and shared root constants.
LEGACY_PATHS = {
    "HERDR_ENV_VAR": "utils",
    "HERDR_ENV_VALUE": "utils",
    "api::server": "server",
    "api::event_hub": "server",
    "api::subscriptions": "server",
    "api::wait": "server",
    "api::prompt_wait": "server",
    "api::ApiRequestMessage": "server",
    "api::ApiRequestSender": "server",
    "api::EventHub": "server",
    "api::ServerHandle": "server",
    "api::api_method_name": "server",
    "api::request_changes_ui": "server",
    "api::start_server_with_stop_control": "server",
    "bus::entry": "cli",
    "bus::control_cli": "cli",
    "bus::control_focus_cli_tests": "cli",
    "input::lease": "client",
    "kitty_graphics::surface": "server",
    "pane::state": "server",
    "pane::PaneState": "server",
    "server::autodetect": "cli",
    "server::socket_paths": "utils",
    "terminal::id": "utils",
    "terminal::TerminalId": "utils",
    "terminal::title": "agents",
    "terminal::stripped_terminal_title": "agents",
    "terminal::runtime::state": "server",
    "terminal::runtime::PaneState": "server",
    "ui::status": "utils",
    "ui::text": "utils",
    "ui::widgets": "utils",
    "ui::render_config_diagnostic_buffer": "utils",
    "ui::render_copy_feedback_buffer": "utils",
}
EXCEPTIONS = {"src/server/tests/render_scale.rs": frozenset({"client"})}
IDENTIFIER = r"(?:r#)?[A-Za-z_]\w*"
PATH = re.compile(rf"{IDENTIFIER}(?:\s*::\s*{IDENTIFIER})*")
CRATE_PATH = re.compile(rf"\bcrate\s*::\s*(?P<path>{PATH.pattern})")
CRATE_GROUP = re.compile(r"\bcrate\s*::\s*\{")


def component(module: str) -> str | None:
    for prefix in sorted(LEGACY_PATHS, key=len, reverse=True):
        if module == prefix or module.startswith(prefix + "::"):
            return LEGACY_PATHS[prefix]
    root = module.split("::", 1)[0]
    return root if root in DEPENDENCIES else LEGACY_ROOTS.get(root)


def source_component(path: Path) -> str | None:
    if path.as_posix() == "src/main.rs":
        return "main"
    if path.as_posix() == "src/compat_paths.rs":
        return None
    return component("::".join(path.with_suffix("").parts[1:]))


def crate_paths(code: str):
    for match in CRATE_PATH.finditer(code):
        yield match.start(), re.sub(r"\s+|r#", "", match["path"])
    # Root-braced use trees have no segment immediately after crate::.
    for group in CRATE_GROUP.finditer(code):
        depth, start = 1, group.end()
        for index in range(start, len(code)):
            token = code[index]
            depth += (token == "{") - (token == "}")
            if (token == "," and depth == 1) or depth == 0:
                entry = PATH.match(
                    code,
                    start + len(code[start:index]) - len(code[start:index].lstrip()),
                )
                if entry and entry.end() <= index:
                    yield entry.start(), re.sub(r"\s+|r#", "", entry[0])
                start = index + 1
            if depth == 0:
                break


def allowed(owner: str, target: str, path: Path) -> bool:
    return (
        owner == "main"
        or target in {owner, "utils"}
        or target in DEPENDENCIES[owner]
        or target in EXCEPTIONS.get(path.as_posix(), ())
    )


@dataclass(frozen=True)
class Reference:
    path: Path
    line: int
    owner: str
    target: str
    module: str

    @property
    def forbidden(self) -> bool:
        return self.target not in DEPENDENCIES or not allowed(
            self.owner, self.target, self.path
        )

    def diagnostic(self) -> str:
        return f"{self.path.as_posix()}:{self.line}: {self.owner} -> {self.target} (crate::{self.module})"


def scan(root: Path = PROJECT_ROOT) -> tuple[int, list[Reference]]:
    paths = sorted((root / "src").rglob("*.rs"))
    if not paths:
        raise ValueError(f"No Rust sources found under {root / 'src'}")
    references = []
    checked = 0
    for path in paths:
        relative = path.relative_to(root)
        owner = source_component(relative)
        if relative.as_posix() == "src/compat_paths.rs":
            continue
        if owner is None:
            raise ValueError(
                f"Unmapped source owner: {relative.as_posix()}; update LEGACY_ROOTS"
            )
        checked += 1
        code = mask_comments_and_literals(path.read_text(encoding="utf-8"))
        for offset, module in sorted(crate_paths(code)):
            references.append(
                Reference(
                    relative,
                    code.count("\n", 0, offset) + 1,
                    owner,
                    component(module) or "unknown",
                    module,
                )
            )
    return checked, references


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=PROJECT_ROOT)
    parser.add_argument(
        "--enforce", action="store_true", help="fail on forbidden edges"
    )
    args = parser.parse_args(argv)
    try:
        checked, references = scan(args.root)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    violations = [ref for ref in references if ref.forbidden]
    edges = {(ref.owner, ref.target) for ref in references if ref.owner != ref.target}
    forbidden_edges = {(ref.owner, ref.target) for ref in violations}
    print(
        f"Import boundaries: {checked} Rust files, {len(edges)} component edges, "
        f"{len(forbidden_edges)} forbidden edges, {len(violations)} forbidden references"
    )
    for owner, target in sorted(edges):
        print(f"  {owner} -> {target}")
    for ref in violations:
        print(ref.diagnostic())
    return int(args.enforce and bool(violations))


if __name__ == "__main__":
    raise SystemExit(main())
