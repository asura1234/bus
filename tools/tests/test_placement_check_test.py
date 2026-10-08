from pathlib import Path

import pytest

from tools.quality import placement


def _write(root: Path, path: str, source: str):
    destination = root / path
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(source)
    return destination


def test_test_paths_and_test_only_cfg_are_allowed(tmp_path):
    _write(tmp_path, "src/main.rs", '''#[cfg(all(test, unix))]
#[path = "helper.rs"] mod helper;
#[cfg(test)] mod inline {
    #[tokio::test] async fn works() {}
}
fn main() {}
''')
    _write(tmp_path, "src/helper.rs", '#[path = "suite_test.rs"] mod suite;')
    _write(tmp_path, "src/suite_test.rs", "#[test] fn works() {}")
    _write(tmp_path, "tools/acceptance/driver_test.py", "def verify(): pass")
    assert placement.check(tmp_path) == []


@pytest.mark.parametrize("attribute", ("test", "tokio::test", "should_panic"))
def test_rust_test_attributes_outside_scopes_are_rejected(tmp_path, attribute):
    _write(tmp_path, "src/main.rs", f"#[{attribute}] fn unplaced() {{}}")
    assert any("Rust test body outside" in issue for issue in placement.check(tmp_path))


def test_combined_production_cfg_is_not_an_exemption(tmp_path):
    _write(tmp_path, "src/main.rs", "#[cfg(any(windows, test))] #[test] fn unplaced() {}")
    assert any("Rust test body outside" in issue for issue in placement.check(tmp_path))


@pytest.mark.parametrize("edge", (
    'mod suite_test;', '#[path = "suite_test.rs"] mod renamed;',
    'include!("suite_test.rs");', 'include!(r#"suite_test.rs"#);',
))
def test_production_module_and_include_edges_cannot_reach_test_files(tmp_path, edge):
    _write(tmp_path, "src/main.rs", edge)
    _write(tmp_path, "src/suite_test.rs", "fn helper() {}")
    assert any("production" in issue and "test file" in issue for issue in placement.check(tmp_path))


def test_path_graph_checks_nested_inline_and_transitive_modules(tmp_path):
    _write(tmp_path, "src/main.rs", "mod nested { mod bridge; }")
    _write(tmp_path, "src/nested/bridge.rs", '#[path = "../suite_test.rs"] mod suite;')
    _write(tmp_path, "src/suite_test.rs", "fn helper() {}")
    assert any("production module edge" in issue for issue in placement.check(tmp_path))


def test_a_test_directory_is_not_itself_a_production_reachability_guard(tmp_path):
    _write(tmp_path, "src/main.rs", '#[path = "tests/bridge_test.rs"] mod bridge;')
    _write(tmp_path, "src/tests/bridge_test.rs", "fn helper() {}")
    assert any("production module edge" in issue for issue in placement.check(tmp_path))


def test_cargo_target_roots_are_checked_even_without_main(tmp_path):
    _write(tmp_path, "Cargo.toml", '[[bin]]\nname="app"\npath="src/app.rs"\n')
    _write(tmp_path, "src/app.rs", "mod suite_test;")
    _write(tmp_path, "src/suite_test.rs", "fn helper() {}")
    assert any("production module edge" in issue for issue in placement.check(tmp_path))


def test_cfg_test_include_edges_and_black_box_targets_remain_test_only(tmp_path):
    _write(tmp_path, "src/main.rs", '#[cfg(test)] mod tests { include!("helper_test.rs"); }')
    _write(tmp_path, "src/helper_test.rs", "#[test] fn unit() {}")
    _write(tmp_path, "Cargo.toml", '[[test]]\nname="api"\npath="tests/api/api_test.rs"\n')
    _write(tmp_path, "tests/api/api_test.rs", "mod support_test; #[test] fn api() {}")
    _write(tmp_path, "tests/api/support_test.rs", "fn helper() {}")
    assert placement.check(tmp_path) == []


def test_computed_includes_fail_closed_until_audited(tmp_path):
    _write(tmp_path, "src/main.rs", 'include!(concat!(env!("OUT_DIR"), "/code.rs"));')
    assert any("computed include!" in issue for issue in placement.check(tmp_path))


@pytest.mark.parametrize("source", (
    "def test_unplaced(): pass",
    "class TestUnplaced: pass",
    "import unittest\nclass QualityTest(unittest.TestCase): pass",
    "from unittest import TestCase as Case\nclass QualityTest(Case): pass",
    "from unittest import TestCase\nclass Base(TestCase): pass\nclass Derived(Base): pass",
))
def test_python_ast_finds_functions_classes_and_unittest_aliases(tmp_path, source):
    _write(tmp_path, "skills/example/scripts/checker.py", source)
    assert any("Python test body outside" in issue for issue in placement.check(tmp_path))


def test_comments_string_fixtures_and_production_predicates_are_ignored(tmp_path):
    _write(tmp_path, "src/main.rs", '// #[test] fn fake() {}\nconst TEXT: &str = "#[test]";')
    _write(tmp_path, "tools/checker.py", '# def test_fake(): pass\nTEXT = "class TestFake: pass"\ndef is_test(path): return False')
    assert placement.check(tmp_path) == []


def test_suffix_naming_is_required_even_inside_test_directories(tmp_path):
    _write(tmp_path, "src/deep/tests/helper.rs", "fn helper() {}")
    _write(tmp_path, "skills/example/scripts/test_missed.py", "def test_case(): pass")
    issues = placement.check(tmp_path)
    assert sum("_test suffix" in issue for issue in issues) == 2
    assert any("Python test body outside" in issue for issue in issues)


def test_cli_reports_without_enforcement_and_can_fail_closed(tmp_path, capsys):
    _write(tmp_path, "tools/checker.py", "def test_unplaced(): pass")
    assert placement.main(["--root", str(tmp_path)]) == 0
    assert "1 violation(s)" in capsys.readouterr().out
    assert placement.main(["--root", str(tmp_path), "--enforce"]) == 1
