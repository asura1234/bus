"""Read exact source references using the repository's Rust syntax helpers."""
from __future__ import annotations

import json
import re
import subprocess
import sys
import tomllib
from collections import defaultdict
from pathlib import Path

from suite_python_paths import python_edges

LITERAL = r'r(?P<hash>\#*)"(?P<raw>.*?)"(?P=hash)|"(?:\\.|[^"\\])*"'
QUOTED = re.compile(LITERAL, re.S)
INCLUDE = re.compile(r'\binclude(?:_str|_bytes)?\s*!\s*\(\s*(' + LITERAL + ')', re.S)
MODULE_EDGE = re.compile(r'\bmod\s+\w+\s*;|\binclude\s*!\s*\(')
MANIFEST_PATH = re.compile(r'\bconcat\s*!\s*\(\s*env\s*!\s*\(\s*"CARGO_MANIFEST_DIR"\s*\)\s*,\s*(' + LITERAL + r')\s*,?\s*\)', re.S)


def owner(name: str) -> tuple[str, ...]:
    parts = Path(name).parts
    return parts[:2] if parts[0] in {"src", "tests", "skills"} else parts[:1]


def literal(text: str) -> str | None:
    if text.startswith('r'):
        match = QUOTED.fullmatch(text)
        return match.group('raw') if match else None
    try:
        return json.loads(text)
    except ValueError:
        return None


class References:
    def __init__(self, root: Path):
        self.root = root.resolve()
        if (root / '.git').exists():
            result = subprocess.run(['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z'],
                                    cwd=root, capture_output=True, check=True)
            self.paths = {item.decode(errors='surrogateescape') for item in result.stdout.split(b'\0') if item}
        else:
            self.paths = {path.relative_to(root).as_posix() for path in root.rglob('*') if path.is_file() or path.is_symlink()}
        self.edges: dict[str, set[str]] = defaultdict(set)
        self.prefixes: list[tuple[str, str]] = []
        self.sources: dict[str, str] = {}
        self.uncertain = False
        engine = root if (root / 'tools/quality/placement.py').is_file() else Path(__file__).resolve().parents[3]
        added = str(engine) not in sys.path
        if added:
            sys.path.insert(0, str(engine))
        try:
            from tools.quality.placement import rust_edges
            from tools.quality.rust_source import mask_comments_and_literals
        finally:
            if added:
                sys.path.remove(str(engine))
        self.rust_edges = rust_edges
        self.mask_rust = mask_comments_and_literals
        for name in sorted(self.paths):
            path = root / name
            if Path(name).parts[0] in {'vendor', 'temp', 'target'} or path.suffix not in {'.rs', '.py'} or not path.is_file():
                continue
            source = path.read_text(encoding='utf-8')
            self.sources[name] = source
            if path.suffix == '.rs':
                self.rust(name, source)
            else:
                self.python(name, source)
        self.bundle_aliases()
        manifest = root / 'Cargo.toml'
        data = tomllib.loads(manifest.read_text()) if manifest.exists() else {}
        self.targets = {target['name']: target['path'] for target in data.get('test', []) if 'path' in target}

    def relative(self, path: Path) -> str | None:
        path = path.resolve()
        return path.relative_to(self.root).as_posix() if path.is_relative_to(self.root) else None

    def rust(self, name: str, source: str):
        code = self.mask_rust(source)
        if MODULE_EDGE.search(code):
            try:
                for edge in self.rust_edges(self.root / name, source):
                    target = self.relative(edge.target)
                    if target:
                        self.edges[name].add(target)
            except ValueError:
                self.uncertain = True
        for match in INCLUDE.finditer(source):
            # Text in comments or fixture strings is not an include expression.
            if not code[match.start():].startswith('include'):
                continue
            value = literal(match.group(1))
            if value is not None:
                target = self.relative(self.root / Path(name).parent / value)
                if target:
                    self.edges[name].add(target)
        self.runtime_files(name, source, code)
        if name == 'build.rs':
            # Rebuild watches do not mean every watched file enters the binary.
            # Only directories actually passed to the resource collector do.
            for variable, directory in re.findall(r'let\s+(\w+)\s*=\s*root\.join\("([^"]+)"\)', source):
                if re.search(r'\bcollect\s*\(\s*&' + re.escape(variable) + r'\s*,', source):
                    target = self.relative(self.root / directory)
                    if target:
                        self.prefixes.append((name, target))
                    else:
                        self.uncertain = True

    def runtime_files(self, name: str, source: str, code: str):
        text = self.mask_rust(source, mask_literals=False)
        candidates = []
        for match in QUOTED.finditer(text):
            value = literal(match.group())
            if value and '/' in value and not value.startswith('/') and '\n' not in value and '\0' not in value:
                candidates.extend((self.root / value, self.root / Path(name).parent / value))
        for match in MANIFEST_PATH.finditer(text):
            if code[match.start():].startswith('concat'):
                value = literal(match.group(1))
                if value is not None:
                    candidates.append(self.root / value.lstrip('/'))
        for path in candidates:
            try:
                target = self.relative(path)
                if target in self.paths and path.is_file():
                    self.edges[name].add(target)
            except (OSError, ValueError):
                continue

    def python(self, name: str, source: str):
        targets = python_edges(self.root, name, source, self.paths)
        if targets is None:
            self.uncertain = True
        else:
            self.edges[name].update(targets)

    @staticmethod
    def resource_input(name: str, prefix: str) -> bool:
        if name != prefix and not name.startswith(prefix + '/'):
            return False
        relative = Path(name).relative_to(prefix)
        return 'tests' not in relative.parts and not relative.name.endswith('_test.py')

    def bundle_aliases(self):
        pending, seen = list(self.prefixes), set()
        while pending:
            source, prefix = pending.pop()
            if (source, prefix) in seen:
                continue
            seen.add((source, prefix))
            for name in self.paths:
                path = self.root / name
                if not self.resource_input(name, prefix) or not path.is_symlink():
                    continue
                target = self.relative(path)
                if target:
                    if path.is_dir():
                        self.prefixes.append((source, target))
                        pending.append((source, target))
                    else:
                        self.edges[source].add(target)

    def incoming(self, name: str) -> set[str]:
        return {source for source, targets in self.edges.items() if name in targets} | {
            source for source, prefix in self.prefixes if self.resource_input(name, prefix)
        }

    def cross_owner(self, name: str) -> bool:
        pending, seen = [name], set()
        while pending:
            target = pending.pop()
            if target in seen:
                continue
            seen.add(target)
            for source in self.incoming(target):
                if owner(source) != owner(name):
                    return True
                # Python helpers can be imported by a script launched from Rust.
                # Rust uses namespace selection plus integration-target reachability.
                if name.endswith('.py'):
                    pending.append(source)
        return False

    def reachable(self, start: str) -> set[str]:
        seen, pending = set(), [start]
        while pending:
            name = pending.pop()
            if name in seen:
                continue
            seen.add(name)
            pending.extend(self.edges.get(name, ()))
        return seen

    def integration_targets(self, name: str) -> set[str]:
        return {target for target, start in self.targets.items() if name in self.reachable(start)}

    def needs_binary(self, tests: list[str]) -> bool:
        reachable = set().union(*(self.reachable(name) for name in tests)) if tests else set()
        return any('BUS_TEST_BINARY' in self.sources.get(name, '') or
                   'target/debug/bus' in self.sources.get(name, '') for name in reachable)
