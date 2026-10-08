from __future__ import annotations

import re
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[2]
# Stage integration updates these globs as their owners move/split Rust files.
SOURCES = {
    "hot_path": (
        "src/ui.rs",
        "src/ui/**/*.rs",
        "src/server/render_stream.rs",
        "src/utils/render/widgets.rs",
        "src/utils/render/status_popups.rs",
        "src/utils/text/width.rs",
    ),
    "app_server": (
        "src/app/**/*.rs",
        "src/server/**/*.rs",
        "src/utils/socket_paths.rs",
    ),
}
TEST_MODULE = re.compile(
    r"(?m)^[ \t]*#\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*"
    r"(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{"
)
CHAR_LITERAL = re.compile(r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^\\'\n])'")
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


def blank_non_newlines(chars: list[str], start: int, end: int) -> None:
    for index in range(start, end):
        if chars[index] != "\n":
            chars[index] = " "


def mask_comments_and_literals(source: str) -> str:
    chars = list(source)
    index = 0
    while index < len(source):
        if source.startswith("//", index):
            end = source.find("\n", index + 2)
            end = len(source) if end == -1 else end
            blank_non_newlines(chars, index, end)
            index = end
            continue

        if source.startswith("/*", index):
            depth = 1
            end = index + 2
            while end < len(source) and depth > 0:
                if source.startswith("/*", end):
                    depth += 1
                    end += 2
                elif source.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
            blank_non_newlines(chars, index, end)
            index = end
            continue

        if source[index] == "r":
            quote = index + 1
            while quote < len(source) and source[quote] == "#":
                quote += 1
            if quote < len(source) and source[quote] == '"':
                suffix = '"' + "#" * (quote - index - 1)
                end = source.find(suffix, quote + 1)
                end = len(source) if end == -1 else end + len(suffix)
                blank_non_newlines(chars, index, end)
                index = end
                continue

        if source[index] == '"':
            end = index + 1
            while end < len(source):
                if source[end] == "\\":
                    end += 2
                elif source[end] == '"':
                    end += 1
                    break
                else:
                    end += 1
            blank_non_newlines(chars, index, min(end, len(source)))
            index = end
            continue

        if source[index] == "'" and (literal := CHAR_LITERAL.match(source, index)):
            blank_non_newlines(chars, index, literal.end())
            index = literal.end()
            continue

        index += 1

    return "".join(chars)


def production_code(source: str) -> str:
    code = mask_comments_and_literals(source)
    chars = list(code)
    search_from = 0

    while test_module := TEST_MODULE.search(code, search_from):
        depth = 0
        end = test_module.end() - 1
        while end < len(code):
            if code[end] == "{":
                depth += 1
            elif code[end] == "}":
                depth -= 1
                if depth == 0:
                    end += 1
                    break
            end += 1
        blank_non_newlines(chars, test_module.start(), end)
        code = "".join(chars)
        search_from = end

    return code


def source_paths(group: str, root: Path = PROJECT_ROOT) -> tuple[Path, ...]:
    paths = {
        path
        for pattern in SOURCES[group]
        for path in root.glob(pattern)
        if path.is_file()
        and path.suffix == ".rs"
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
