from __future__ import annotations

import re
from pathlib import Path

from tools.quality.rust_source import mask_comments_and_literals as mask_comments_and_literals
from tools.quality.scopes import is_test_path, production_code


PROJECT_ROOT = Path(__file__).resolve().parents[2]
# Stage integration updates these globs as their owners move/split Rust files.
SOURCES = {
    "hot_path": (
        "src/server/rendering/surface/**/*.rs",
        "src/server/workspaces/agent_panel.rs",
        "src/server/rendering/stream.rs",
        "src/utils/render/widgets.rs",
        "src/utils/render/status_popups.rs",
        "src/utils/text/width.rs",
    ),
    "app_server": (
        "src/cli/launch.rs",
        "src/server/mod.rs",
        "src/server/**/*.rs",
        "src/utils/socket_paths.rs",
    ),
}
INPUT_STATE_CALL = re.compile(r"(?:\.|::)input_state\b")
KEYBOARD_STATE_ANSI_CALL = re.compile(
    r"(?:\.|::)(?:keyboard_state_ansi|kitty_keyboard_state_ansi)\b"
)
AGGREGATE_STATE_CALLS = (
    (INPUT_STATE_CALL, "aggregate terminal input state; add a narrow accessor"),
    (KEYBOARD_STATE_ANSI_CALL, "formatted keyboard state"),
)
FORBIDDEN_CALLS = (
    *AGGREGATE_STATE_CALLS,
    (
        re.compile(r"(?:\.|::)screen_text_snapshot\b"),
        "formatted terminal screen snapshot",
    ),
    (
        re.compile(r"\bforeground_job\s*\("),
        "process-tree inspection",
    ),
)


def source_paths(group: str, root: Path = PROJECT_ROOT) -> tuple[Path, ...]:
    paths = {
        path
        for pattern in SOURCES[group]
        for path in root.glob(pattern)
        if path.is_file()
        and path.suffix == ".rs"
        and not is_test_path(path.relative_to(root))
        and "tests" not in path.relative_to(root / "src").parts
        and path.name != "tests.rs"
        and not path.stem.endswith("_tests")
    }
    if not paths:
        raise ValueError(f"No {group} Rust sources discovered from {SOURCES[group]}")
    return tuple(sorted(paths))


def find_violations(paths, rules, root: Path = PROJECT_ROOT) -> list[str]:
    violations: list[str] = []
    for path in paths:
        code = production_code(path.read_text(encoding="utf-8"))
        for pattern, description in rules:
            for match in pattern.finditer(code):
                line = code.count("\n", 0, match.start()) + 1
                relative_path = path.relative_to(root)
                violations.append(f"{relative_path}:{line}: {description}")
    return violations


def check(root: Path = PROJECT_ROOT) -> list[str]:
    return [
        *find_violations(source_paths("hot_path", root), FORBIDDEN_CALLS, root),
        *find_violations(source_paths("app_server", root), AGGREGATE_STATE_CALLS, root),
    ]
