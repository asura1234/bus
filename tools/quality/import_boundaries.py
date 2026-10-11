"""Report Rust dependencies against the final physical component graph.

All cfg branches and test scopes are inspected. Literal module/include edges,
use trees, lexical bindings and re-export chains are resolved from source. This
is not compiler resolution: associated items, type inference and procedural
macro expansion still need compiler/integrator verification. New declarative
macros fail closed pending an explicit audit; known generators are listed below.
"""
from __future__ import annotations

import argparse
import hashlib
import re
import tomllib
from dataclasses import dataclass
from pathlib import Path

from tools.quality.hot_path import PROJECT_ROOT
from tools.quality.placement import rust_edges
from tools.quality.rust_source import mask_comments_and_literals
from tools.quality.scopes import balanced_pairs, in_ranges, is_test_path, rust_test_ranges

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
    "devtools": frozenset({"terminal", "messaging", "protocol", "platform"}),
}
EXCEPTIONS = {"src/server/tests/render_scale_test.rs": frozenset({"client"})}
IDENTIFIER = r"(?:r#)?[A-Za-z_]\w*"
PATH = re.compile(rf"{IDENTIFIER}(?:\s*::\s*{IDENTIFIER})*")
USE = re.compile(r"\buse\s+([^;]+);", re.S)
MODULE = re.compile(rf"\bmod\s+({IDENTIFIER})\s*([;{{])")
DECLARATION = re.compile(rf"(?<!')\b(fn|struct|enum|union|type|trait|const(?!\s+(?:unsafe\s+)?fn\b)|static(?:\s+mut)?)\s+({IDENTIFIER})")
MACRO = re.compile(rf"\bmacro_rules\s*!\s*({IDENTIFIER})\s*([{{(\[])")
TOKEN = re.compile(rf"{IDENTIFIER}|::|[{{}},*]")

# Audited at S12: id_type declares exactly its identifier argument; writer_handle
# adds Clone/Drop impls to existing types; fallback/success contain literal paths
# that the ordinary scanner checks. None constructs an import/module path.
AUDITED_MACROS = {
    ("src/messaging/model/types.rs", "id_type"): "dcab1820c082b997ca94a629b76d6fc979e2d4f7271df93b9ba2c53189753e04",
    ("src/server/clients/writer.rs", "writer_handle"): "aa905ca5628c1b0be4eb05fa8a923f7f278aaeedeac6275fa4c2b8f8e1bebfa4",
    ("src/server/rendering/incremental.rs", "fallback"): "82bc435eddd3faed430e2603761b9f8fb1b2c9b328c01d76e171c70388a73992",
    ("src/server/rendering/incremental.rs", "success"): "4d15be9a37a2c6b395627c54c7a8db08ffef7b2cd1cf2c8eab606bbb4300b561",
}


def audited_macro(source, match):
    end = source.pairs.get(match.end() - 1, len(source.code) - 1)
    body = re.sub(r"\s+", "", source.code[match.start():end + 1])
    digest = hashlib.sha256(body.encode()).hexdigest()
    return AUDITED_MACROS.get((source.path.as_posix(), match[1])) == digest


def component(module: str) -> str | None:
    root = module.split("::", 1)[0]
    return root if root in DEPENDENCIES else None


def source_component(path: Path) -> str | None:
    if path.as_posix() == "src/main.rs":
        return "main"
    if len(path.parts) >= 3 and path.parts[0] == "src":
        return component(path.parts[1])
    return None


def allowed(owner: str, target: str, path: Path, *, test_only: bool = False) -> bool:
    if owner == "main":
        return target in DEPENDENCIES or target == "main"
    if target not in DEPENDENCIES:
        return False
    if test_only and target in EXCEPTIONS.get(path.as_posix(), ()):
        return True
    if not (target in {owner, "utils"} or target in DEPENDENCIES.get(owner, ())):
        return False
    if target == "agents" and path.parts[:3] in {
        ("src", "terminal", "vt"), ("src", "terminal", "pty"),
        ("src", "terminal", "emulator"),
    }:
        return False
    return not (owner == "server" and target == "messaging"
                and path.as_posix() != "src/server/terminals/resume.rs")


def _parts(text: str) -> tuple[str, ...]:
    return tuple(re.sub(r"\s+|r#", "", text).split("::"))


def _physical_module(path: Path) -> tuple[str, ...]:
    if path.as_posix() == "src/main.rs":
        return ()
    parts = path.with_suffix("").parts[1:]
    return tuple(parts[:-1] if parts[-1] == "mod" else parts)


def use_leaves(code: str, offset: int = 0):
    """Expand nested use trees, retaining each leaf's original source offset."""
    tokens = [(m[0].removeprefix("r#"), offset + m.start()) for m in TOKEN.finditer(code)]
    index = 0

    def tree(prefix):
        nonlocal index
        parts = list(prefix)
        start = tokens[index][1] if index < len(tokens) else offset
        while index < len(tokens):
            token, _ = tokens[index]
            index += 1
            if token == "::":
                continue
            if token == "{":
                while index < len(tokens) and tokens[index][0] != "}":
                    yield from tree(tuple(parts))
                    if index < len(tokens) and tokens[index][0] == ",":
                        index += 1
                if index >= len(tokens):
                    raise ValueError("Unbalanced Rust use tree")
                index += 1
                return
            if token in {",", "}"}:
                index -= 1
                break
            if token == "as":
                if index >= len(tokens):
                    raise ValueError("Missing Rust use alias")
                alias = tokens[index][0]
                index += 1
                if len(parts) > 1 and parts[-1] == "self":
                    parts.pop()
                yield start, tuple(parts), alias
                return
            parts.append(token)
            if index >= len(tokens) or tokens[index][0] in {",", "}"}:
                break
        if parts:
            # foo::{self, Item} binds foo itself, not a member named self.
            if len(parts) > 1 and parts[-1] == "self":
                parts.pop()
            yield start, tuple(parts), parts[-1] if parts else "self"

    yield from tree(())
    if index != len(tokens):
        raise ValueError("Unsupported Rust use tree")


def crate_paths(code: str):
    """Return fully expanded crate use paths and explicit expression paths."""
    spans = []
    for use in USE.finditer(code):
        spans.append(use.span())
        for offset, parts, _ in use_leaves(use[1], use.start(1)):
            if parts and parts[0] == "crate":
                yield offset, "::".join(parts[1:])
    for match in PATH.finditer(code):
        parts = _parts(match[0])
        if parts[0] == "crate" and len(parts) > 1 and not in_ranges(match.start(), spans):
            yield match.start(), "::".join(parts[1:])


@dataclass(frozen=True)
class Import:
    offset: int
    parts: tuple[str, ...]
    alias: str
    scope: tuple[int, int]
    module: tuple[str, ...]


class Source:
    def __init__(self, path: Path, text: str, base: tuple[str, ...]):
        self.path, self.text, self.base = path, text, base
        self.code = mask_comments_and_literals(text)
        self.pairs = balanced_pairs(self.code)
        self.blocks = [(a, b) for a, b in self.pairs.items() if self.code[a] == "{"]
        self.modules = []
        for match in MODULE.finditer(self.code):
            if match[2] == "{":
                opening = match.end() - 1
                self.modules.append((opening, self.pairs[opening], match[1].removeprefix("r#")))
        self.test_ranges = rust_test_ranges(text)
        self.imports, self.use_spans = [], []
        for match in USE.finditer(self.code):
            self.use_spans.append(match.span())
            for offset, parts, alias in use_leaves(match[1], match.start(1)):
                self.imports.append(Import(offset, parts, alias, self.scope(match.start()),
                                           self.module(match.start())))

    def module(self, offset):
        return self.base + tuple(name for a, b, name in sorted(self.modules)
                                 if a < offset < b)

    def scope(self, offset):
        return max(((a, b) for a, b in self.blocks if a < offset < b),
                   default=(-1, len(self.code)), key=lambda pair: pair[0])

    def module_scope(self, offset):
        return max(((a, b) for a, b, _ in self.modules if a < offset < b),
                   default=(-1, len(self.code)), key=lambda pair: pair[0])


@dataclass(frozen=True)
class Destination:
    module: tuple[str, ...]
    path: Path | None


class Registry:
    def __init__(self, root: Path, texts: dict[Path, str]):
        self.sources = []
        self.modules, self.symbols, self.bindings, self.globs = {}, {}, {}, {}
        self.edges = []
        self.externals = {"std", "core", "alloc"}
        manifest = root / "Cargo.toml"
        if manifest.is_file():
            def dependencies(table):
                for key, value in table.items():
                    if key in {"dependencies", "dev-dependencies", "build-dependencies"}:
                        self.externals.update(name.replace("-", "_") for name in value)
                    elif isinstance(value, dict):
                        dependencies(value)
            dependencies(tomllib.loads(manifest.read_text()))
        visited, visiting = set(), set()

        def visit(path, base, directory):
            key = (path, base, directory)
            if key in visited:
                return
            if path in visiting:
                raise ValueError(f"Cyclic Rust module/include edge: {path}")
            visiting.add(path)
            visited.add(key)
            source = Source(path, texts[path], base)
            self.sources.append(source)
            self.modules.setdefault(base, set()).add(path)
            for a, _, _ in source.modules:
                self.modules.setdefault(source.module(a + 1), set()).add(path)
            for edge in rust_edges(root / path, texts[path], directory):
                target = edge.target.relative_to(root)
                if target not in texts:
                    raise ValueError(f"Unresolved owned Rust {edge.kind} edge: {path} -> {target}")
                self.edges.append((source, edge.offset, target, edge.test_only))
                module = source.module(edge.offset)
                if edge.kind == "module":
                    match = MODULE.match(source.code, edge.offset)
                    module += (match[1].removeprefix("r#"),)
                visit(target, module, edge.module_dir)
            visiting.remove(path)

        if Path("src/main.rs") in texts:
            visit(Path("src/main.rs"), (), root / "src")
        for path in sorted(texts):
            if not any(key[0] == path for key in visited):
                directory = (root / path).parent if path.name == "mod.rs" else (root / path).with_suffix("")
                visit(path, _physical_module(path), directory)
        for source in self.sources:
            for match in DECLARATION.finditer(source.code):
                scope = source.scope(match.start())
                if scope != source.module_scope(match.start()) and not re.search(
                    r"\bextern\s*$", source.code[max(0, scope[0] - 80):scope[0]]
                ):
                    continue
                name = source.module(match.start()) + (match[2].removeprefix("r#"),)
                self.symbols.setdefault(name, set()).add((source.path, match[1]))
            id_generator = any(match[1] == "id_type" and audited_macro(source, match)
                               for match in MACRO.finditer(source.code))
            for match in re.finditer(r"\bid_type!\s*\(\s*(\w+)\s*\)", source.code):
                if id_generator:
                    name = source.module(match.start()) + (match[1],)
                    self.symbols.setdefault(name, set()).add((source.path, "struct"))
            for imp in source.imports:
                if imp.scope == source.module_scope(imp.offset):
                    if imp.alias == "*":
                        self.globs.setdefault(imp.module, []).append((source, imp))
                    elif imp.alias != "_":
                        self.bindings.setdefault(imp.module + (imp.alias,), []).append((source, imp))

    def names(self, module):
        return {name[len(module)] for table in (self.modules, self.symbols, self.bindings)
                for name in table if len(name) == len(module) + 1 and name[:len(module)] == module}

    def exports(self, destination, seen=frozenset()):
        result = {destination}
        module = destination.module
        if module in seen:
            return result
        seen = seen | {module}
        for child in self.names(module):
            result.update(self.resolve(module + (child,)))
        for source, imp in self.globs.get(module, ()):
            for target in self.local(imp.parts[:-1], source, imp.offset):
                if target.path is not None:
                    result.update(self.exports(target, seen))
                else:
                    result.add(target)
        return result

    def resolve(self, name, seen=frozenset()):
        if name in seen:
            return {Destination(name, None)}
        seen = seen | {name}
        # Root composition aliases cannot turn an obsolete root into a component.
        if name and name[0] not in DEPENDENCIES and name not in self.symbols:
            return {Destination(name, None)}
        for size in range(1, len(name) + 1):
            prefix, tail = name[:size], name[size:]
            if prefix in self.bindings and not (tail and prefix in self.modules):
                # Rust permits a module and a value re-export with the same
                # spelling (restore::restore, parse::parse). A qualified member
                # uses the module namespace, not the value binding.
                result = {Destination(name, path) for path in self.modules.get(name, ())}
                for source, imp in self.bindings[prefix]:
                    # Consuming an owned facade is an edge even when its final
                    # export comes from an external crate. Keep both facade and
                    # underlying physical owners so neither can hide an edge.
                    result.add(Destination(prefix, source.path))
                    key = ("binding", source.path, imp.offset)
                    if key in seen:
                        result.add(Destination(name, None))
                    else:
                        result.update(self.local(imp.parts + tail, source, imp.offset, seen | {key}, strict=True))
                return result
        if name in self.modules:
            return {Destination(name, path) for path in self.modules[name]}
        for size in range(len(name), 0, -1):
            prefix = name[:size]
            if prefix in self.symbols:
                result = {Destination(prefix, path) for path, kind in self.symbols[prefix]
                          if size == len(name) or kind in {"struct", "enum", "union", "trait", "type"}}
                return result or {Destination(name, None)}
        # Once a namespace exists, its ancestors' globs cannot supply members of
        # that namespace. Mark each traversed glob too: expanding a self-reexport
        # must not manufacture ever longer unresolved names or recurse forever.
        size = next((size for size in range(len(name) - 1, -1, -1)
                     if name[:size] in self.modules), 0)
        result = set()
        for source, imp in self.globs.get(name[:size], ()):
            key = ("glob", source.path, imp.offset)
            if key in seen:
                continue
            for destination in self.local(imp.parts[:-1], source, imp.offset, seen | {key}):
                if destination.path is not None:
                    result.update(self.resolve(destination.module + name[size:], seen | {key}))
        if any(destination.path is not None for destination in result):
            # A glob branch lacking this name is not an unresolved import: a
            # different cfg/export branch may supply it. Explicit named aliases
            # keep their unknown destinations and still fail closed.
            return {destination for destination in result if destination.path is not None}
        return {Destination(name, None)}

    def local(self, parts, source, offset, seen=frozenset(), *, strict=False):
        if not parts:
            return {Destination((), None)}
        if parts[-1] == "*":
            result = set()
            for destination in self.local(parts[:-1], source, offset, seen, strict=strict):
                if destination.path is not None:
                    exports = self.exports(destination)
                    # An included test's logical parent can be declared in a
                    # different component solely to preserve its test identity.
                    # Relative globs import the parent's symbols, not that
                    # composition container. Keep every symbol's physical owner.
                    own_context = parts[0] in {"self", "super"} and source.path in self.modules.get(destination.module, ())
                    result.update(target for target in exports
                                  if not own_context or target.module != destination.module)
                else:
                    result.add(destination)
            return result
        module = source.module(offset)
        if parts[0] == "crate":
            return self.resolve(parts[1:], seen)
        if parts[0] in {"self", "super"}:
            index = 0
            while index < len(parts) and parts[index] in {"self", "super"}:
                if parts[index] == "super":
                    if not module:
                        return {Destination(parts, None)}
                    module = module[:-1]
                index += 1
            return self.resolve(module + parts[index:], seen)
        imports = [imp for imp in source.imports if imp.alias == parts[0]
                   and imp.offset != offset and imp.scope[0] < offset < imp.scope[1]
                   and not (len(parts) > 1 and module + (parts[0],) in self.modules)]
        if imports:
            scope = max(imp.scope[0] for imp in imports)
            result = set()
            for imp in imports:
                if imp.scope[0] == scope:
                    key = ("alias", source.path, imp.offset)
                    if key in seen:
                        result.add(Destination(parts, None))
                    else:
                        result.update(self.local(imp.parts + parts[1:], source, imp.offset, seen | {key}, strict=True))
            return result
        candidate = module + parts
        if candidate in self.modules or any(candidate[:size] in table for size in range(len(module) + 1, len(candidate) + 1)
                                            for table in (self.modules, self.symbols, self.bindings)):
            return self.resolve(candidate, seen)
        if module in self.globs:
            result = self.resolve(candidate, seen)
            if any(destination.path is not None for destination in result):
                return result
        if parts[0] in DEPENDENCIES:
            return self.resolve(parts, seen)
        # External crate imports do not belong to the owned component graph.
        if not strict or parts[0] in self.externals:
            return set()
        return {Destination(candidate, None)}


@dataclass(frozen=True)
class Reference:
    path: Path
    line: int
    owner: str
    target: str
    module: str
    test_only: bool = False
    reason: str = ""

    @property
    def forbidden(self) -> bool:
        return bool(self.reason) or not allowed(self.owner, self.target, self.path,
                                               test_only=self.test_only)

    def diagnostic(self) -> str:
        suffix = f"; {self.reason}" if self.reason else ""
        return f"{self.path.as_posix()}:{self.line}: {self.owner} -> {self.target} (crate::{self.module}){suffix}"


def scan(root: Path = PROJECT_ROOT) -> tuple[int, list[Reference]]:
    root = root.resolve()
    paths = sorted((root / "src").rglob("*.rs"))
    if not paths:
        raise ValueError(f"No Rust sources found under {root / 'src'}")
    texts = {}
    for path in paths:
        relative = path.relative_to(root)
        if source_component(relative) is None:
            raise ValueError(f"Unmapped source owner: {relative}; final component directory required")
        texts[relative] = path.read_text(encoding="utf-8")
    registry = Registry(root, texts)
    references = set()
    for source in registry.sources:
        owner = source_component(source.path)

        def record(offset, destinations, reason=""):
            for destination in destinations:
                target = source_component(destination.path) if destination.path else "unknown"
                references.add(Reference(source.path, source.code.count("\n", 0, offset) + 1,
                                         owner, target, "::".join(destination.module),
                                         is_test_path(source.path) or in_ranges(offset, source.test_ranges),
                                         reason))

        # Test module/include wiring is traversed for context and imports; it is
        # not itself a product dependency. Production cross-owner wiring is.
        for origin, offset, target, test_edge in registry.edges:
            if origin is source and not (test_edge or is_test_path(source.path)) and source_component(target) != owner:
                record(offset, {Destination(_physical_module(target), target)})

        for imp in source.imports:
            record(imp.offset, registry.local(imp.parts, source, imp.offset, strict=True))
        for match in PATH.finditer(source.code):
            if in_ranges(match.start(), source.use_spans):
                continue
            parts = _parts(match[0])
            if len(parts) > 1 or any(
                imp.alias == parts[0] for imp in source.imports
            ):
                if source.code[max(0, match.start() - 1):match.start()] == ".":
                    continue
                record(match.start(), registry.local(parts, source, match.start()))
        for match in MACRO.finditer(source.code):
            if not audited_macro(source, match):
                record(match.start(), {Destination(source.module(match.start()) + (match[1],), source.path)},
                       "macro-generated paths require an explicit audit")
    return len(texts), sorted(references, key=lambda ref: (ref.path.as_posix(), ref.line, ref.module, ref.target, ref.reason))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=PROJECT_ROOT)
    parser.add_argument("--enforce", action="store_true", help="fail on forbidden edges")
    args = parser.parse_args(argv)
    try:
        checked, references = scan(args.root)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    violations = [ref for ref in references if ref.forbidden]
    edges = {(ref.owner, ref.target) for ref in references if ref.owner != ref.target}
    forbidden_edges = {(ref.owner, ref.target) for ref in violations}
    print(f"Import boundaries: {checked} Rust files, {len(edges)} component edges, "
          f"{len(forbidden_edges)} forbidden edges, {len(violations)} forbidden references")
    for owner, target in sorted(edges):
        print(f"  {owner} -> {target}")
    for ref in violations:
        print(ref.diagnostic())
    return int(args.enforce and bool(violations))


if __name__ == "__main__":
    raise SystemExit(main())
