from tools.quality.rust_source import mask_comments_and_literals
from tools.quality.scopes import (
    cfg_requires_test,
    is_test_path,
    production_code,
    production_line_count,
    rust_test_ranges,
    rust_test_line_numbers,
)

import pytest


@pytest.mark.parametrize("path", (
    "x_test.rs", "tools/acceptance/live_ui_test.py", "src/a/tests/b/c.rs",
    "tests/a.rs", "a/tests/deeper/helper.py", "a\\tests\\helper.rs",
))
def test_shared_path_scopes_at_every_depth(path):
    assert is_test_path(path)


@pytest.mark.parametrize("path", (
    "src/contest/a.rs", "src/tests_extra/a.rs", "src/a_tests.rs",
    "src/tests.rs", "tools/test_checker.py", "src/test_helpers/a.rs",
))
def test_shared_path_scope_near_misses(path):
    assert not is_test_path(path)


@pytest.mark.parametrize("expression, expected", (
    ("test", True), ("all(test, unix)", True), ("any(windows, test)", False),
    ("not(test)", False), ("not(not(test))", True),
    ('all(any(test, feature = "x"), not(feature = "x"))', True),
    ("all()", False), ("any()", True),
))
def test_cfg_implication_handles_balanced_boolean_expressions(expression, expected):
    assert cfg_requires_test(expression) is expected


def test_masker_preserves_offsets_literals_lifetimes_and_nested_comments():
    source = r'''/* outside /* #[test] */ more */
const BYTES: &[u8] = b"#[test] { }";
const RAW: &[u8] = br##"#[cfg(test)] } }"##;
const CHARACTER: char = '\u{7b}';
fn production<'a>(input: &'a str) { use crate::protocol; }
'''
    code = mask_comments_and_literals(source)
    assert len(code) == len(source)
    assert code.count("\n") == source.count("\n")
    assert "#[test]" not in code and "#[cfg" not in code
    assert "'a" in code and "crate::protocol" in code


def test_union_scopes_cover_nested_helpers_impls_fields_and_match_arms():
    source = '''fn before() {}
#[cfg(all(test, unix))]
mod helpers {
    #[cfg(test)] fn nested() {}
    const BRACE: &str = r#"}}"#;
}
#[cfg(test)]
impl Foo { fn helper(&self) {} }
struct Foo {
    production: i32,
    #[cfg(test)] helper: Option<(i32, i32)>,
}
fn arm(n: i32) { match n {
    #[cfg(test)] 1 => { helper(); },
    _ => production(),
} }
#[cfg(any(windows, test))] fn production_windows() {}
fn after() {}
'''
    code = production_code(source)
    for removed in ("helpers", "nested", "impl Foo", "helper", "1 =>"):
        assert removed not in code
    for kept in ("before", "production: i32", "_ => production()", "production_windows", "after"):
        assert kept in code
    ranges = rust_test_ranges(source)
    assert len(ranges) == 4
    assert len(code) == len(source)
    assert code.count("\n") == source.count("\n")


def test_cfg_attr_is_conditional_and_inner_cfg_covers_its_container():
    source = '''mod unit {
    #![cfg(test)]
    fn helper() {}
}
#[cfg_attr(not(test), cfg(any()))] fn other_helper() {}
#[cfg_attr(test, allow(dead_code))] fn production() {}
'''
    code = production_code(source)
    assert "helper" not in code
    assert "fn production()" in code


def test_physical_line_counter_keeps_comments_blanks_and_adjacent_production():
    source = '''// production comment

#[cfg(test)]
fn helper() {
    // test comment
}
fn production() {} #[cfg(test)] fn adjacent_helper() {}
'''
    assert production_line_count(source) == 3
    assert production_line_count(source, "deep/tests/helper.rs") == 0
    assert production_line_count(source, "helper_test.rs") == 0


def test_path_defaults_can_be_supplied_from_the_eventual_policy():
    assert is_test_path("a/spec/case.rs", test_dirs=("spec",))
    assert is_test_path("a/case.spec.rs", test_files=("*.spec.rs",), test_dirs=())
    assert not is_test_path("a/tests/case.rs", test_files=(), test_dirs=())


def test_production_counter_and_coverage_share_wholly_test_lines():
    source = "fn before() {}\n#[cfg(all(test, unix))]\nmod cases {\n    fn helper() {}\n}\nfn after() {} #[cfg(test)] fn adjacent() {}\n"
    assert rust_test_line_numbers(source) == frozenset({2, 3, 4, 5})
    assert production_line_count(source) == 2
    assert production_line_count(source, "cases.spec.rs", test_files=("*.spec.rs",)) == 0
    assert production_line_count(source, "a/spec/case.rs", test_dirs=("spec",)) == 0
    assert production_line_count(source, "a/tests/case.rs", test_dirs=()) == 2


def test_match_arm_patterns_and_comparisons_do_not_hide_following_production():
    source = '''fn decide(value: Foo) { match value {
    #[cfg(test)] Foo { n } if n < limit => n < limit,
    _ => production(),
} }
#[cfg(test)] fn generic<const N: usize>() -> Foo<{N + 1}> { helper() }
fn after() {}
'''
    code = production_code(source)
    assert "n < limit" not in code
    assert "generic" not in code
    assert "_ => production()" in code
    assert "fn after()" in code
