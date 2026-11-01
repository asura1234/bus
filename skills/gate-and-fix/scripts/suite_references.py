"""Read source references without importing or executing repository code.

The index is conservative: ambiguous literal paths can add owners, never remove
them. Computed/unknown inputs are left to the selector's full-suite fallback.
"""
from __future__ import annotations

import ast
import json
import re
import subprocess
import tomllib
from collections import defaultdict
from pathlib import Path

LITERAL = r'r(?P<hash>\#{0,8})"(?P<raw>.*?)"(?P=hash)|"(?:\\.|[^"\\])*"'
QUOTED = re.compile(LITERAL, re.S)
INCLUDE = re.compile(r'\binclude(?:_str|_bytes)?\s*!\s*\(\s*(' + LITERAL + ')', re.S)
PATH = re.compile(r'#\s*\[\s*path\s*=\s*(' + LITERAL + r')\s*\]', re.S)
MODULE = re.compile(r'\bmod\s+(\w+)\s*;')


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
            self.paths = {path.relative_to(root).as_posix() for path in root.rglob('*') if path.is_file()}
        self.edges: dict[str, set[str]] = defaultdict(set)
        self.by_name: dict[str, set[str]] = defaultdict(set)
        for name in self.paths:
            self.by_name[Path(name).name].add(name)
        self.prefixes: list[tuple[str, str]] = []
        self.sources: dict[str, str] = {}
        self.uncertain = False
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
        manifest = root / 'Cargo.toml'
        data = tomllib.loads(manifest.read_text()) if manifest.exists() else {}
        self.targets = {target['name']: target['path'] for target in data.get('test', []) if 'path' in target}

    def relative(self, path: Path) -> str | None:
        path = path.resolve()
        return path.relative_to(self.root).as_posix() if path.is_relative_to(self.root) else None

    def path_reference(self, source: str, value: str, *, rust: bool = False, directory: bool = False):
        if not value or '\n' in value or '\x00' in value:
            return
        if rust:
            target = self.relative(self.root / Path(source).parent / value)
            if target:
                self.edges[source].add(target)
        normalized = value.replace('\\', '/').removeprefix('./')
        suffix = normalized.lstrip('./')
        for target in self.by_name.get(Path(suffix).name, ()):
            if target == suffix or target.endswith('/' + suffix):
                self.edges[source].add(target)
        # Directory literals such as ROOT / "skills" are real cross-root inputs.
        if directory and normalized and not normalized.startswith(('.', '/')) and (self.root / normalized).is_dir():
            self.prefixes.append((source, normalized.rstrip('/')))

    def rust(self, name: str, source: str):
        for pattern in (PATH, INCLUDE):
            for match in pattern.finditer(source):
                value = literal(match.group(1))
                if value is not None:
                    self.path_reference(name, value, rust=True)
        for module in MODULE.findall(source):
            path = Path(name)
            bases = (path.parent, path.with_suffix('')) if path.name not in {'mod.rs', 'main.rs', 'lib.rs'} else (path.parent,)
            for base in bases:
                for candidate in (base / f'{module}.rs', base / module / 'mod.rs'):
                    if candidate.as_posix() in self.paths:
                        self.edges[name].add(candidate.as_posix())
        if name == 'build.rs':
            for watched in re.findall(r'cargo:rerun-if-changed=([^"{}\n]+)', source):
                self.prefixes.append((name, watched.rstrip('/')))

    def python(self, name: str, source: str):
        try:
            tree = ast.parse(source)
        except SyntaxError:
            self.uncertain = True
            return
        parents = {child: node for node in ast.walk(tree) for child in ast.iter_child_nodes(node)}

        def parts(node):
            if isinstance(node, ast.Constant) and isinstance(node.value, str):
                return [node.value]
            if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div):
                return parts(node.left) + parts(node.right)
            if isinstance(node, ast.Call):
                return [value for arg in node.args for value in parts(arg)]
            return []

        for node in ast.walk(tree):
            if isinstance(node, ast.Constant) and isinstance(node.value, str):
                self.path_reference(name, node.value)
            elif isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div):
                parent = parents.get(node)
                if not (isinstance(parent, ast.BinOp) and isinstance(parent.op, ast.Div)):
                    self.path_reference(name, '/'.join(parts(node)), directory=True)
            elif isinstance(node, ast.Call) and ((isinstance(node.func, ast.Name) and node.func.id in {'Path', 'PurePath'}) or
                                                (isinstance(node.func, ast.Attribute) and node.func.attr in {'join', 'joinpath'})):
                if not isinstance(parents.get(node), ast.BinOp):
                    self.path_reference(name, '/'.join(parts(node)), directory=True)
            elif isinstance(node, (ast.Import, ast.ImportFrom)):
                modules = ([alias.name for alias in node.names] if isinstance(node, ast.Import) else
                           [node.module or '', *((node.module + '.' if node.module else '') + alias.name for alias in node.names)])
                for module in modules:
                    path = module.replace('.', '/')
                    candidates = {path + '.py', path + '/__init__.py'}
                    if isinstance(node, ast.ImportFrom) and node.level:
                        parent = Path(name).parent
                        for _ in range(node.level - 1):
                            parent = parent.parent
                        candidates.update((parent / candidate).as_posix() for candidate in tuple(candidates))
                    self.edges[name].update(self.paths.intersection(candidates))
                    if '/' not in path:
                        self.edges[name].update(self.by_name.get(path + '.py', ()))

    def incoming(self, name: str) -> set[str]:
        return {source for source, targets in self.edges.items() if name in targets} | {
            source for source, prefix in self.prefixes if name == prefix or name.startswith(prefix + '/')
        }

    def cross_owner(self, name: str) -> bool:
        return any(owner(source) != owner(name) for source in self.incoming(name))

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
