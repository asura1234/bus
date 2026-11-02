"""Reject clock-dependent Rust directory fixtures; exercise the source policy itself.

This bounded lexical analysis follows local values, use aliases and helper returns,
not Rust types or macro expansion. Ambiguous manual PID+clock temporary paths fail
closed. Direct file/socket evidence is accepted only without directory evidence.
There are no per-file exemptions. Use the shared allocator for opaque helpers.
"""
from dataclasses import dataclass
from pathlib import Path
import re

import pytest

from tools.quality.import_boundaries import use_leaves
from tools.quality.placement import rust_edges
from tools.quality.rust_source import CHAR_LITERAL, mask_comments_and_literals
from tools.quality.scopes import balanced_pairs, in_ranges, is_test_path, rust_items, rust_test_ranges


TOKEN = re.compile(CHAR_LITERAL.pattern + r'|b?r(?P<hash>\#*)"[\s\S]*?"(?P=hash)|b?"(?:\\.|[^"\\])*"|'
                   r"(?:r#)?[A-Za-z_]\w*|\d[\w]*|::|->|=>|==|!=|<=|>=|[^\s]", re.S)
IDENT = re.compile(r"(?:r#)?[A-Za-z_]\w*\Z")
DIRECTORIES = {"create_dir", "create_dir_all", "remove_dir", "remove_dir_all",
               "create_private_directory", "prepare_fixture_home", "canonical_directory",
               "current_dir", "set_current_dir"}
DIRECTORY_ENV = {"HOME", "USERPROFILE", "BUS_DATA_DIR", "XDG_CONFIG_HOME",
                 "XDG_RUNTIME_DIR", "XDG_STATE_HOME", "TMPDIR"}
FILES = {"write", "write_atomic", "atomic_write", "remove_file", "bind", "save",
         "bind_local_listener", "bind_private_local_listener", "connect_local_stream"}


@dataclass(frozen=True)
class Value:
    flags: frozenset[str] = frozenset()
    paths: frozenset[tuple[int, int]] = frozenset()  # (origin token, child depth)
    temporary: bool = False
    literal: str = ""


def combined(values):
    values = tuple(values)
    return Value(frozenset().union(*(v.flags for v in values)),
                 frozenset().union(*(v.paths for v in values)),
                 any(v.temporary for v in values),
                 next((v.literal for v in values if v.literal), ""))


class FixtureSource:
    def __init__(self, path, source):
        self.path, self.source = path, source
        self.code = mask_comments_and_literals(source)
        self.tokens = list(TOKEN.finditer(mask_comments_and_literals(source, mask_literals=False)))
        self.words = [m[0] for m in self.tokens]
        offsets = {m.start(): i for i, m in enumerate(self.tokens)}
        self.pairs = {offsets[a]: offsets[b] for a, b in balanced_pairs(self.code).items()
                      if a in offsets and b in offsets}
        self.blocks = [(a, b) for a, b in self.pairs.items() if self.words[a] == "{"]
        self.ranges = (((0, len(source)),) if is_test_path(path) else
                       (*rust_test_ranges(source), *((item.start, item.end) for item in rust_items(source)
                                                    if item.is_test_body)))
        self.imports = []
        for use in re.finditer(r"\buse\s+([^;]+);", self.code):
            scope = self.scope(offsets.get(use.start(), 0))
            for _, parts, alias in use_leaves(use[1]):
                self.imports.append((alias, "::".join(parts), scope))
        self.bindings, self.functions = [], []
        for i, word in enumerate(self.words):
            if word in {"let", "static", "const"}:
                name = i + 1 + (self.words[i + 1:i + 2] == ["mut"])
                if name >= len(self.words) or not IDENT.fullmatch(self.words[name]):
                    continue
                end = self.statement_end(name + 1)
                equal = next((j for j in range(name + 1, end) if self.words[j] == "="), None)
                if equal is not None:
                    static_atomic = word == "static" and any(
                        self.words[j].startswith("Atomic") for j in range(name + 1, equal))
                    self.bindings.append((self.words[name], i, self.scope(i), equal + 1, end, static_atomic))
            elif word == "fn" and i + 2 < len(self.words):
                j = i + 2
                while j < len(self.words) and self.words[j] not in {"(", ";", "{"}:
                    j += 1
                if j not in self.pairs or self.words[j] != "(":
                    continue
                parameters = []
                for a, b in self.arguments(j + 1, self.pairs[j]):
                    colon = next((k for k in range(a, b) if self.words[k] == ":"), None)
                    parameters.append(self.words[colon - 1] if colon else "self")
                j = self.pairs[j] + 1
                while j < len(self.words) and self.words[j] not in {"{", ";"}:
                    j += 1
                if j in self.pairs and self.words[j] == "{":
                    self.functions.append((self.words[i + 1], i, self.scope(i), j, self.pairs[j], tuple(parameters)))
        self.reassignments = {}
        for i in range(1, len(self.words) - 1):
            # A simple local assignment begins a statement. Fields, declarations,
            # comparisons and compound operators are not local-variable writes.
            if (IDENT.fullmatch(self.words[i]) and self.words[i - 1] in {"{", ";", "}"}
                    and self.words[i + 1] == "="):
                binding = self.binding(self.words[i], i)
                if binding is not None and not binding[5]:
                    self.reassignments.setdefault(binding[1], []).append(
                        (i, self.scope(i), i + 2, self.statement_end(i + 2)))
        self.memo, self.active = {}, set()
        self.parameters = []
        self.parent_statics = set()
        self.origins, self.directory_uses, self.file_uses = set(), set(), set()

    def scope(self, token):
        return max(((a, b) for a, b in self.blocks if a < token < b),
                   default=(-1, len(self.words)), key=lambda p: p[0])

    def statement_end(self, start):
        j = start
        while j < len(self.words) and self.words[j] not in {";", "}"}:
            j = self.pairs[j] + 1 if j in self.pairs else j + 1
        return j

    def arguments(self, start, end):
        pieces, j, first = [], start, start
        while j < end:
            if self.words[j] == ",":
                pieces.append((first, j))
                first = j + 1
            j = self.pairs[j] + 1 if j in self.pairs else j + 1
        if first < end:
            pieces.append((first, end))
        return pieces

    def binding(self, name, offset):
        return max((b for b in self.bindings if b[0] == name and (b[4] < offset or b[5])
                    and b[2][0] < offset < b[2][1]),
                   default=None, key=lambda b: (b[2][0], b[1]))

    def canonical(self, name, offset):
        first, *tail = name.split("::")
        imports = [imp for imp in self.imports if imp[0] == first and imp[2][0] < offset < imp[2][1]]
        if imports:
            first = max(imports, key=lambda imp: imp[2][0])[1]
        return "::".join((first, *tail))

    def local_value(self, name, offset):
        binding = self.binding(name, offset)
        for first, last, parameter, value in reversed(self.parameters):
            if parameter == name and first < offset < last and (binding is None or binding[1] < first):
                return value
        if binding is None:
            return Value()
        writes = tuple(write for write in self.reassignments.get(binding[1], ()) if write[3] < offset)
        key = ("binding", binding[1], writes, tuple(self.parameters))
        def current_value():
            value = self.value(binding[3], binding[4])
            for _, scope, start, end in writes:
                updated = self.value(start, end)
                if scope == binding[2]:
                    value = updated
                else:
                    # A nested block may be conditional. Preserve either possible
                    # clock/PID value, but sequence safety must hold for both.
                    flags = value.flags | updated.flags
                    if "sequence" not in value.flags & updated.flags:
                        flags -= {"sequence"}
                    merged = combined((value, updated))
                    value = Value(flags, merged.paths, merged.temporary, merged.literal)
            return value
        return self.cached(key, current_value)

    def cached(self, key, calculate):
        if key in self.active:
            return Value()
        if key not in self.memo:
            self.active.add(key)
            self.memo[key] = calculate()
            self.active.remove(key)
        return self.memo[key]

    def block_value(self, start, end):
        # Only the tail/explicit return supplies a helper's value; unrelated
        # statements and counters in its body cannot add sequence protection.
        tails, first, j = [], start, start
        while j < end:
            if self.words[j] == "return":
                tails.append((j + 1, min(end, self.statement_end(j + 1))))
            if self.words[j] == ";":
                first = j + 1
            j = self.pairs[j] + 1 if j in self.pairs else j + 1
        if first < end:
            tails.append((first, end))
        return combined(self.value(a, b) for a, b in tails)

    def effect(self, value, kind):
        target = self.directory_uses if kind == "directory" else self.file_uses
        target.update(origin for origin, depth in value.paths if kind == "directory" or depth == 0)

    def candidate(self, token, value):
        if {"pid", "clock"} <= value.flags and "sequence" not in value.flags:
            if in_ranges(self.tokens[token].start(), self.ranges):
                self.origins.add(token)
                return frozenset({(token, 0)})
        return frozenset()

    def call(self, name, args, offset):
        leaf = name.split("::")[-1]
        value = combined(args)
        if name.endswith("process::id"):
            return Value(frozenset({"pid"}))
        if leaf in {"now_ns", "now_ms", "now_us"} or name.endswith("SystemTime::now"):
            return Value(frozenset({"clock"}))
        if name.endswith("env::temp_dir"):
            return Value(temporary=True)
        if leaf == "env" and value.literal == "CARGO_MANIFEST_DIR":
            return Value(frozenset({"manifest"}))
        if leaf in DIRECTORIES and args:
            self.effect(args[0], "directory")
        if leaf in FILES or name.endswith("File::create"):
            if args:
                self.effect(args[0], "file")
        if leaf == "set_var" and len(args) > 1 and args[0].literal in DIRECTORY_ENV:
            self.effect(args[1], "directory")
        if name.endswith("PathBuf::from") or name.endswith("Path::new"):
            temporary = (value.temporary or value.literal in {"/tmp", "/private/tmp"}
                         or value.literal.startswith(("/tmp/", "/private/tmp/")))
            paths = value.paths | (self.candidate(offset, value) if temporary and not value.paths else frozenset())
            return Value(value.flags, paths, temporary, value.literal)
        if leaf in {"format", "format_args"}:
            captures = []
            for match in re.finditer(r"(?<!\{)\{([A-Za-z_]\w*)(?:[}:])", value.literal):
                captures.append(self.local_value(match[1], offset))
            return combined((value, *captures))
        functions = [f for f in self.functions if f[0] == leaf and f[2][0] < offset < f[2][1]]
        if functions:
            function = max(functions, key=lambda f: f[2][0])
            def returned():
                previous = self.parameters
                self.parameters = [*previous, *((function[3], function[4], parameter, argument)
                                                for parameter, argument in zip(function[5], args))]
                try:
                    return self.block_value(function[3] + 1, function[4])
                finally:
                    self.parameters = previous
            return self.cached(("function", function[1], tuple(args)), returned)
        return value

    def chain(self, start, end):
        word, j = self.words[start], start + 1
        receiver_name = word
        if word.startswith(('"', 'b"', 'r"', 'r#', 'br"', 'br#')) and '"' in word:
            literal = word[word.index('"') + 1:word.rfind('"')]
            value = Value(literal=literal)
        elif word in {"(", "[", "{"} and start in self.pairs:
            close = self.pairs[start]
            value = self.block_value(start + 1, close) if word == "{" else self.value(start + 1, close)
            j = close + 1
        elif IDENT.fullmatch(word):
            while j + 1 < end and self.words[j] == "::" and IDENT.fullmatch(self.words[j + 1]):
                receiver_name += "::" + self.words[j + 1]
                j += 2
            name = self.canonical(receiver_name, start)
            if j < end and self.words[j] == "!":
                j += 1
            if j < end and self.words[j] in {"(", "["} and j in self.pairs:
                close = self.pairs[j]
                args = [self.value(a, b) for a, b in self.arguments(j + 1, close)]
                value, j = self.call(name, args, start), close + 1
            else:
                value = self.local_value(word, start) if "::" not in receiver_name else Value()
        else:
            return Value(), j
        while j + 2 < end and self.words[j] == "." and IDENT.fullmatch(self.words[j + 1]):
            method, opening = self.words[j + 1], j + 2
            if self.words[opening] != "(" or opening not in self.pairs:
                break
            close = self.pairs[opening]
            args = [self.value(a, b) for a, b in self.arguments(opening + 1, close)]
            if method == "join" and args:
                self.effect(value, "directory")
                paths = frozenset((origin, depth + 1) for origin, depth in value.paths)
                if value.temporary:
                    paths |= self.candidate(start, args[0])
                temporary = value.temporary or ("manifest" in value.flags and args[0].literal == "temp")
                value = Value(value.flags | args[0].flags, paths, temporary)
            elif method == "parent":
                value = Value(value.flags, frozenset((origin, depth - 1) for origin, depth in value.paths
                                                     if depth > 0), value.temporary)
            elif method == "fetch_add":
                binding = self.binding(receiver_name, start)
                static = binding[5] if binding else receiver_name in self.parent_statics
                first = self.arguments(opening + 1, close)
                unit_increment = first and self.words[first[0][0]:first[0][1]] == ["1"]
                if static and unit_increment:
                    value = Value(frozenset({"sequence"}))
                else:
                    value = Value()
            else:
                if method == "current_dir" and args:
                    self.effect(args[0], "directory")
                elif method in FILES and args:
                    self.effect(args[0], "file")
                # Conversions, borrowing wrappers, Result unwrap/expect and clock
                # conversions retain receiver provenance, not message arguments.
            j = close + 1
        return value, j

    def value(self, start, end):
        values, j = [], start
        while j < end:
            value, j = self.chain(j, end)
            values.append(value)
        return combined(values)

    def violations(self):
        for binding in self.bindings:
            self.local_value(binding[0], binding[4] + 1)
        # Start at each expression root; nested calls are evaluated recursively.
        # Function bodies are also visited independently of helper call reachability.
        for i, word in enumerate(self.words):
            if IDENT.fullmatch(word) and (i == 0 or self.words[i - 1] not in {".", "::"}):
                self.chain(i, len(self.words))
            if word == "default_shell" and self.words[i + 1:i + 2] == ["="]:
                self.effect(self.value(i + 2, self.statement_end(i + 2)), "file")
        return [f"{self.path}:{self.source.count(chr(10), 0, self.tokens[i].start()) + 1}: "
                "PID + wall clock temporary directory needs the shared sequence allocator"
                for i in sorted(self.origins)
                if i in self.directory_uses or i not in self.file_uses]


def fixture_directory_violations(sources):
    # Include static-only parent modules so an actually imported counter can
    # supply sequence provenance. Never search unrelated files by counter name.
    models = {path: FixtureSource(path, source) for path, source in sources.items()
              if any(word in source for word in ("temp_dir", "/tmp", "CARGO_MANIFEST_DIR", "fetch_add"))
              or ("static" in source and "Atomic" in source)}
    root = Path(__file__).resolve().parents[2]
    for path, parent in models.items():
        statics = [b for b in parent.bindings if b[5]]
        if not statics:
            continue
        for edge in rust_edges(root / path, parent.source):
            child = models.get(str(edge.target.relative_to(root)))
            if child is None or not any(alias == "*" and name == "super::*" for alias, name, _ in child.imports):
                continue
            declaration = next((i for i, t in enumerate(parent.tokens) if t.start() == edge.offset), None)
            if declaration is not None:
                child.parent_statics.update(b[0] for b in statics if b[2] == parent.scope(declaration))
    return [violation for path in sorted(models) for violation in models[path].violations()]


CLOCK = "std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()"
NAME = 'format!("sample-{}-{}", std::process::id(), ' + CLOCK + ')'
PATH = "std::env::temp_dir().join(" + NAME + ")"


@pytest.mark.parametrize("consumer", (
    "std::fs::create_dir_all(&root).unwrap();",
    'let file = root.join("session.json"); std::fs::write(file, b"{}").unwrap();',
    "std::process::Command::new(\"true\").current_dir(&root);",
    'std::env::set_var("XDG_CONFIG_HOME", &root);',
    "canonical_directory(&root.to_string_lossy()).unwrap_err();",
    "assert!(!root.exists());",
))
def test_rejects_directory_consumers_and_unproven_missing_paths(consumer):
    source = "fn test() { let root = " + PATH + "; " + consumer + " }"
    assert len(fixture_directory_violations({"tests/case.rs": source})) == 1


def test_aliases_and_format_captures_keep_clock_and_pid_provenance():
    source = '''use std::{env::temp_dir as scratch, process::id as process,
        time::{SystemTime as Wall, UNIX_EPOCH as EPOCH}};
fn make() {
    let timestamp = Wall::now().duration_since(EPOCH).unwrap().as_millis();
    let pid = process();
    let root = scratch().join(format!("example-{pid}-{timestamp}"));
    std::fs::create_dir_all(&root).unwrap();
}'''
    assert len(fixture_directory_violations({"src/unit/tests/helpers_test.rs": source})) == 1


def test_helper_returned_directory_and_fixed_clock_callers_are_rejected():
    source = '''fn now_ns() -> u128 { 7 }
fn named_path() -> PathBuf {
    std::env::temp_dir().join(format!("sample-{}-{}", std::process::id(), now_ns()))
}
fn test() { let path = named_path(); let child = path.join("socket"); }
'''
    assert len(fixture_directory_violations({"tests/helper.rs": source})) == 1


def test_file_beneath_timestamp_named_parent_is_a_directory_fixture():
    source = 'fn make() -> PathBuf { ' + PATH + '.join("session.json") }'
    assert len(fixture_directory_violations({"tests/helper.rs": source})) == 1


def test_unrelated_atomic_and_shadowed_names_do_not_protect_a_directory():
    source = '''static NEXT: AtomicU64 = AtomicU64::new(0);
fn unrelated() -> u64 { NEXT.fetch_add(1, Ordering::Relaxed) }
fn test() {
    let serial = NEXT.fetch_add(1, Ordering::Relaxed);
    { let serial = 0; let other = serial; }
    let root = ''' + PATH + ''';
    std::fs::create_dir_all(&root).unwrap();
}'''
    assert len(fixture_directory_violations({"tests/case.rs": source})) == 1


def test_sequence_must_be_in_the_directory_name_and_come_from_a_static():
    source = '''fn test() {
    let counter = AtomicU64::new(0);
    let sequence = counter.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("sample-{}-{}-{sequence}",
        std::process::id(), ''' + CLOCK + '''));
    std::fs::create_dir_all(&root).unwrap();
}'''
    assert len(fixture_directory_violations({"tests/case.rs": source})) == 1


def test_static_sequence_in_the_actual_component_protects_a_directory():
    source = '''static NEXT: AtomicU64 = AtomicU64::new(0);
fn make() -> PathBuf {
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("sample-{}-{}-{sequence}",
        std::process::id(), ''' + CLOCK + '''))
}
fn test() { std::fs::create_dir_all(make()).unwrap(); }'''
    assert fixture_directory_violations({"tests/case.rs": source}) == []


@pytest.mark.parametrize("consumer", (
    'std::fs::write(&root, b"data").unwrap();',
    "let file = std::fs::File::create(&root).unwrap();",
    "let listener = UnixListener::bind(&root).unwrap();",
    "let listener = bind_local_listener(&root).unwrap();",
    "app.state.default_shell = root.to_string_lossy().into_owned();",
))
def test_proven_direct_files_extensionless_sockets_and_executables_are_allowed(consumer):
    source = "fn test() { let root = " + PATH + "; " + consumer + " }"
    assert fixture_directory_violations({"tests/case.rs": source}) == []


def test_file_evidence_does_not_bless_a_mixed_directory_helper():
    source = 'fn test() { let root = ' + PATH + ''';
    let listener = UnixListener::bind(&root).unwrap();
    let child = root.join("state");
}'''
    assert len(fixture_directory_violations({"tests/case.rs": source})) == 1


def test_direct_file_parent_does_not_reclassify_the_file_as_a_directory():
    source = 'fn test() { let root = ' + PATH + ''';
    std::fs::create_dir_all(root.parent().unwrap()).unwrap();
    std::fs::write(root, b"marker").unwrap();
}'''
    assert fixture_directory_violations({"tests/case.rs": source}) == []


def test_nested_scopes_do_not_transfer_file_evidence_to_another_binding():
    source = 'fn test() { let root = ' + PATH + ''';
    { let root = PathBuf::from("/tmp/marker"); std::fs::write(&root, b"marker"); }
    assert!(!root.exists());
}'''
    assert len(fixture_directory_violations({"tests/case.rs": source})) == 1


def test_production_comments_literals_and_scalar_ids_are_outside_policy():
    source = 'fn production() { let root = ' + PATH + '''; fs::create_dir(root); }
#[cfg(test)] mod unit {
    // std::env::temp_dir().join(format!("{}-{}", std::process::id(), now_ns()));
    const EXAMPLE: &str = r#"fn fake() { temp_dir().join(now_ns()) }"#;
    fn test() {
        let request_id = format!("{}-{}", std::process::id(), now_ns());
        let marker = format!("SIZE:{}:{}", std::process::id(), now_ns());
    }
}'''
    assert fixture_directory_violations({"src/component/worker.rs": source}) == []


def test_inline_test_scope_does_not_hide_neighboring_directory_fixtures():
    source = 'fn production() { let root = ' + PATH + '''; fs::create_dir(root); }
#[cfg(all(test, unix))] mod unit {
    fn helper() { let root = ''' + PATH + '''; fs::create_dir_all(&root); }
}
fn after() {}
'''
    violations = fixture_directory_violations({"src/component/worker.rs": source})
    assert len(violations) == 1
    assert violations[0].startswith("src/component/worker.rs:3:")


def test_absolute_tmp_path_constructor_is_also_checked():
    source = '''fn test() {
    let root = PathBuf::from(format!("/tmp/sample-{}-{}", std::process::id(), now_ns()));
    fs::create_dir_all(&root);
}'''
    assert len(fixture_directory_violations({"tests/case.rs": source})) == 1


@pytest.mark.parametrize("base", ('PathBuf::from("/tmp")',
                                    'PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("temp")'))
def test_literal_temp_bases_and_manifest_temp_directories_are_checked(base):
    source = 'fn test() { let root = ' + base + '.join(' + NAME + '); fs::create_dir_all(root); }'
    assert len(fixture_directory_violations({"tests/case.rs": source})) == 1


def test_character_literals_cannot_hide_later_directory_fixtures():
    source = '''fn unrelated() { assert!(!script.contains('"')); }
fn test() { let root = ''' + PATH + '; fs::create_dir_all(root); }'
    assert len(fixture_directory_violations({"tests/case.rs": source})) == 1


def test_parent_static_import_protects_only_the_counter_actually_used():
    parent = '''#[path="child_test.rs"] mod child;
static NEXT: AtomicU64 = AtomicU64::new(0);
'''
    child = '''use super::*;
fn test() { let root = std::env::temp_dir().join(format!("sample-{}-{}-{}",
    std::process::id(), now_ns(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(root);
}'''
    assert fixture_directory_violations({"tests/parent_test.rs": parent,
                                         "tests/child_test.rs": child}) == []
    # A static in another file with the same spelling is not an imported binding.
    assert len(fixture_directory_violations({"tests/unrelated_test.rs": 'static NEXT: AtomicU64 = AtomicU64::new(0);',
                                             "tests/child_test.rs": child})) == 1


def test_clock_provenance_crosses_helper_parameters_and_return_values():
    source = '''fn fixed_clock() -> u128 { now_ns() }
fn make(timestamp: u128) -> PathBuf {
    std::env::temp_dir().join(format!("sample-{}-{timestamp}", std::process::id()))
}
fn test() { let root = make(fixed_clock()); fs::create_dir_all(root); }
'''
    assert len(fixture_directory_violations({"tests/case.rs": source})) == 1


def test_explicit_test_attribute_also_marks_test_scope():
    source = '#[test] fn case() { let root = ' + PATH + '; fs::create_dir_all(root); }'
    assert len(fixture_directory_violations({"src/component/worker.rs": source})) == 1


def test_zero_or_unknown_atomic_increment_cannot_prove_independent_names():
    for increment in ("0", "step"):
        source = '''static NEXT: AtomicU64 = AtomicU64::new(0);
fn test() { let step = 0;
    let root = std::env::temp_dir().join(format!("sample-{}-{}-{}",
        std::process::id(), now_ns(), NEXT.fetch_add(''' + increment + ''', Ordering::Relaxed)));
    fs::create_dir_all(root);
}'''
        assert len(fixture_directory_violations({"tests/case.rs": source})) == 1


@pytest.mark.parametrize("setup, expected", (
    ("let mut timestamp = 0; timestamp = now_ns();", 1),
    ("let mut timestamp = now_ns(); timestamp = 0;", 0),
    ("let mut timestamp = 0; { timestamp = now_ns(); }", 1),
    ("let mut timestamp = now_ns(); if condition { timestamp = 0; }", 1),
    ("let mut timestamp = now_ns(); { let mut timestamp = 0; timestamp = 0; }", 1),
    ("let mut timestamp = 0; { let mut timestamp = now_ns(); timestamp = 0; }", 0),
))
def test_local_reassignments_follow_current_value_and_lexical_binding(setup, expected):
    source = 'fn test() { ' + setup + '''
    let root = std::env::temp_dir().join(format!("sample-{}-{timestamp}", std::process::id()));
    fs::create_dir_all(&root);
}'''
    assert len(fixture_directory_violations({"tests/case.rs": source})) == expected


@pytest.mark.parametrize("operator", ("==", "!=", "<=", ">="))
def test_comparisons_do_not_assign_clock_provenance(operator):
    source = '''fn test() { let timestamp = 0;
    assert!(timestamp ''' + operator + ''' now_ns());
    let root = std::env::temp_dir().join(format!("sample-{}-{timestamp}", std::process::id()));
    fs::create_dir_all(&root);
}'''
    assert fixture_directory_violations({"tests/case.rs": source}) == []


def test_later_assignment_does_not_change_an_earlier_value_snapshot():
    source = '''fn test() { let mut timestamp = now_ns();
    let saved = timestamp; timestamp = 0;
    let root = std::env::temp_dir().join(format!("sample-{}-{saved}", std::process::id()));
    fs::create_dir_all(&root);
}'''
    assert len(fixture_directory_violations({"tests/case.rs": source})) == 1
    source = '''fn test() { let mut timestamp = 0;
    let root = std::env::temp_dir().join(format!("sample-{}-{timestamp}", std::process::id()));
    timestamp = now_ns(); fs::create_dir_all(&root);
}'''
    assert fixture_directory_violations({"tests/case.rs": source}) == []


def test_reassignment_rhs_reads_the_previous_value_of_its_binding():
    source = '''fn test() { let mut root = std::env::temp_dir();
    root = root.join(format!("sample-{}-{}", std::process::id(), now_ns()));
    fs::create_dir_all(&root);
}'''
    assert len(fixture_directory_violations({"tests/case.rs": source})) == 1


def test_repository_fixture_directories_use_independent_sequence_numbers():
    root = Path(__file__).resolve().parents[2]
    sources = {str(path.relative_to(root)): path.read_text(encoding="utf-8")
               for directory in ("src", "tests")
               for path in (root / directory).rglob("*.rs")}
    assert sources, "No Rust sources found for fixture directory policy"
    violations = fixture_directory_violations(sources)
    assert not violations, "Unsafe temporary directory fixtures:\n" + "\n".join(violations)
