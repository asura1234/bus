"""Report test placement and Rust production reachability across owned sources.

This module has no dependency on hot_path and does not enable a gate policy.
"""
from __future__ import annotations

import argparse
import ast
import json
import re
import subprocess
import tomllib
from dataclasses import dataclass
from pathlib import Path

from tools.quality.rust_source import mask_comments_and_literals
from tools.quality.scopes import (
    balanced_pairs,
    in_ranges,
    is_test_path,
    rust_items,
    rust_test_ranges,
)


PROJECT_ROOT = Path(__file__).resolve().parents[2]
OWNED_ROOTS = ("src", "tests", "scripts", "skills", "cli_extensions", "tools", "packaging")
MODULE = re.compile(r"\bmod\s+(\w+)\s*([;{])")
INCLUDE = re.compile(r"\binclude\s*!\s*\(")


def owned_sources(root: Path = PROJECT_ROOT) -> tuple[Path, ...]:
    if (root / ".git").exists():
        names = subprocess.check_output(
            ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=root
        ).decode().split("\0")
        paths = {root / name for name in names if name}
    else:
        paths = set(root.rglob("*.rs")) | set(root.rglob("*.py"))
    return tuple(sorted(
        path for path in paths
        if path.is_file() and path.suffix in {".rs", ".py"}
        and (len(path.relative_to(root).parts) == 1 or path.relative_to(root).parts[0] in OWNED_ROOTS)
    ))


def _message(path: Path, source: str, offset: int, text: str, root: Path) -> str:
    return f"{path.relative_to(root)}:{source.count(chr(10), 0, offset) + 1}: {text}"


def python_test_nodes(source: str) -> tuple[ast.AST, ...]:
    tree = ast.parse(source)
    testcase_names = {"unittest.TestCase"}
    unittest_names = {"unittest"}
    for node in tree.body:
        if isinstance(node, ast.Import):
            unittest_names.update(alias.asname or alias.name for alias in node.names if alias.name == "unittest")
        elif isinstance(node, ast.ImportFrom) and node.module == "unittest":
            testcase_names.update(alias.asname or alias.name for alias in node.names if alias.name == "TestCase")
    testcase_names.update(f"{name}.TestCase" for name in unittest_names)
    nodes = list(ast.walk(tree))
    # Resolve local subclasses too, without importing or executing the source.
    changed = True
    while changed:
        changed = False
        for node in nodes:
            if isinstance(node, ast.ClassDef) and any(ast.unparse(base) in testcase_names for base in node.bases):
                if node.name not in testcase_names:
                    testcase_names.add(node.name)
                    changed = True
    return tuple(
        node for node in nodes
        if (isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name.startswith("test_"))
        or (isinstance(node, ast.ClassDef) and (node.name.startswith("Test") or node.name in testcase_names))
    )


@dataclass(frozen=True)
class RustEdge:
    target: Path
    offset: int
    test_only: bool
    kind: str
    module_dir: Path


def _literal(source: str, start: int) -> tuple[str, int] | None:
    raw = re.match(r'r(#+)?"', source[start:])
    if raw:
        suffix = '"' + (raw[1] or "")
        end = source.find(suffix, start + raw.end())
        return None if end < 0 else (source[start + raw.end():end], end + len(suffix))
    normal = re.match(r'"(?:\\.|[^"\\])*"', source[start:], re.S)
    if normal:
        return json.loads(normal[0]), start + normal.end()
    return None


def _path_value(item) -> str | None:
    if item:
        for attribute in item.attributes:
            if attribute.startswith("path") and (match := re.match(r"path\s*=\s*", attribute)):
                value = _literal(attribute, match.end())
                if value and not attribute[value[1]:].strip():
                    return value[0]
    return None


def rust_edges(path: Path, source: str, module_dir: Path | None = None) -> tuple[RustEdge, ...]:
    """Resolve literal module/include edges, retaining each logical module directory."""
    code = mask_comments_and_literals(source)
    pairs = balanced_pairs(code)
    items = rust_items(source)
    ranges = rust_test_ranges(source)
    headers = {item.header: item for item in items if not item.inner}
    # Header offsets can precede visibility; attach only attributes on this declaration.
    module_items = {}
    for header, item in headers.items():
        match = re.match(r"(?:pub(?:\s*\([^)]*\))?\s+)?(?:unsafe\s+)?mod\s+", code[header:])
        if match:
            offset = code.index("mod", header, header + match.end())
            module_items[offset] = item
    if module_dir is None:
        module_dir = path.parent if path.name in {"mod.rs", "main.rs", "lib.rs", "build.rs"} else path.with_suffix("")
    edges = []

    def visit(start: int, end: int, directory: Path, inline: bool):
        index = start
        for match in MODULE.finditer(code, start, end):
            if match.start() < index:
                continue
            name, delimiter = match.groups()
            item = module_items.get(match.start())
            explicit = _path_value(item)
            if delimiter == "{":
                opening = match.end() - 1
                closing = pairs.get(opening, end)
                child_directory = (directory if inline else path.parent) / explicit if explicit is not None else directory / name
                visit(opening + 1, closing, child_directory, True)
                index = closing + 1
            else:
                if explicit is not None:
                    target = (directory if inline else path.parent) / explicit
                    target_directory = target.parent if target.name == "mod.rs" else target.with_suffix("")
                    # #[path] external modules search unannotated children under their stem.
                else:
                    target = directory / f"{name}.rs"
                    if not target.is_file():
                        target = directory / name / "mod.rs"
                    target_directory = directory / name
                edges.append(RustEdge(target.resolve(), match.start(), in_ranges(match.start(), ranges), "module", target_directory.resolve()))

    visit(0, len(code), module_dir, False)
    for match in INCLUDE.finditer(code):
        opening = match.end() - 1
        end = pairs.get(opening)
        if end is None:
            raise ValueError("unbalanced include! expression")
        content = mask_comments_and_literals(source[match.end():end], mask_literals=False).strip()
        value = _literal(content, 0)
        if value and not content[value[1]:].strip().strip(",").strip():
            target = (path.parent / value[0]).resolve()
            edges.append(RustEdge(target, match.start(), in_ranges(match.start(), ranges), "include", module_dir.resolve()))
        else:
            # Do not silently certify a computed include that the graph cannot resolve.
            raise ValueError("computed include! path requires an explicit source audit")
    return tuple(edges)


def _rust_roots(root: Path, paths: set[Path]) -> tuple[tuple[Path, bool], ...]:
    roots = []
    for name in ("src/main.rs", "src/lib.rs", "build.rs"):
        if (root / name).resolve() in paths:
            roots.append(((root / name).resolve(), False))
    roots.extend((path, False) for path in paths if path.parent == root / "src/bin"
                 or (path.name == "main.rs" and path.parent.parent == root / "src/bin"))
    roots.extend((path, True) for path in paths if path.parent == root / "tests")
    if (root / "Cargo.toml").is_file():
        manifest = tomllib.loads((root / "Cargo.toml").read_text())
        for kind in ("bin", "test", "bench", "example"):
            for target in manifest.get(kind, []):
                if "path" in target:
                    roots.append(((root / target["path"]).resolve(), kind in {"test", "bench"}))
        if "path" in manifest.get("lib", {}):
            roots.append(((root / manifest["lib"]["path"]).resolve(), False))
        build = manifest.get("package", {}).get("build")
        if isinstance(build, str):
            roots.append(((root / build).resolve(), False))
    return tuple(roots)


def rust_reachability(root: Path, sources: dict[Path, str]) -> list[str]:
    violations = []
    seen = set()
    roots = _rust_roots(root, set(sources))
    violations.extend(f"Cargo Rust target source missing from owned inventory: {path}" for path, _ in roots if path not in sources)
    pending = [(path, test, path.parent) for path, test in roots]
    while pending:
        path, test, directory = pending.pop()
        key = (path, test, directory)
        if key in seen or path not in sources:
            continue
        seen.add(key)
        source = sources[path]
        if not test and is_test_path(path.relative_to(root)):
            violations.append(_message(path, source, 0, "test file reachable from a production target", root))
        try:
            edges = rust_edges(path, source, directory)
        except ValueError as error:
            violations.append(_message(path, source, 0, str(error), root))
            continue
        for edge in edges:
            context = test or edge.test_only
            if edge.target not in sources:
                violations.append(_message(path, source, edge.offset, f"unresolved owned Rust {edge.kind} edge: {edge.target}", root))
                continue
            if not context and edge.target in sources and is_test_path(edge.target.relative_to(root)):
                violations.append(_message(path, source, edge.offset, f"production {edge.kind} edge reaches test file {edge.target.relative_to(root)}", root))
            pending.append((edge.target, context, edge.module_dir))
    return violations


def check(root: Path = PROJECT_ROOT, paths=None) -> list[str]:
    root = root.resolve()
    paths = owned_sources(root) if paths is None else tuple(paths)
    violations = []
    rust_sources = {}
    for path in paths:
        path = path.resolve()
        relative = path.relative_to(root)
        source = path.read_text(encoding="utf-8")
        test_path = is_test_path(relative)
        if (test_path or path.name.startswith("test_") or path.stem.endswith("_tests")) and not path.stem.endswith("_test"):
            violations.append(_message(path, source, 0, "test source/helper must use the _test suffix", root))
        if path.suffix == ".rs":
            rust_sources[path] = source
            try:
                ranges = rust_test_ranges(source)
                if not test_path:
                    for item in rust_items(source):
                        if item.is_test_body and not in_ranges(item.header, ranges):
                            violations.append(_message(path, source, item.start, "Rust test body outside a test path or test-only cfg scope", root))
            except ValueError as error:
                violations.append(_message(path, source, 0, str(error), root))
        elif path.suffix == ".py" and not test_path:
            try:
                for node in python_test_nodes(source):
                    violations.append(f"{relative}:{node.lineno}: Python test body outside a test path")
            except SyntaxError as error:
                violations.append(f"{relative}:{error.lineno}: {error.msg}")
    violations.extend(rust_reachability(root, rust_sources))
    return sorted(set(violations))


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=PROJECT_ROOT)
    parser.add_argument("--enforce", action="store_true", help="return a failing exit code for violations")
    arguments = parser.parse_args(argv)
    violations = check(arguments.root)
    for violation in violations:
        print(violation)
    print(f"Test placement: {len(violations)} violation(s)")
    return int(arguments.enforce and bool(violations))


if __name__ == "__main__":
    raise SystemExit(main())
