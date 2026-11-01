"""Reference ownership must follow source edges, including new names and roots."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from suite_selection import select  # noqa: E402


def source(root, name, text):
    path = root / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def test_arbitrary_rust_path_helper_shared_with_a_test_binary_falls_back(tmp_path):
    source(tmp_path, "Cargo.toml", '[[test]]\nname="other"\npath="tests/other/cases.rs"\n')
    source(tmp_path, "src/client/tests/custom_test.rs", "#[test] fn case() {}")
    source(tmp_path, "tests/other/cases.rs", '#[path="../../src/client/tests/custom_test.rs"] mod helper;')
    assert select(["src/client/tests/custom_test.rs"], root=tmp_path).full


def test_new_include_bytes_target_is_not_misclassified_by_its_extension(tmp_path):
    source(tmp_path, "src/main.rs", 'const DATA: &[u8] = include_bytes!("../notes/new.md");')
    source(tmp_path, "notes/new.md", "embedded")
    assert select(["notes/new.md"], root=tmp_path).full


def test_cross_root_python_import_is_a_full_fallback(tmp_path):
    source(tmp_path, "tools/worker.py", "def run(): return 1")
    source(tmp_path, "packaging/check_test.py", "from tools.worker import run\n")
    assert select(["tools/worker.py"], root=tmp_path).full


def test_python_script_paths_are_references_even_without_imports(tmp_path):
    source(tmp_path, "skills/new/scripts/work.py", "print('work')")
    source(tmp_path, "tools/work_test.py", 'SCRIPT = ROOT / "skills" / "new/scripts/work.py"\n')
    assert select(["skills/new/scripts/work.py"], root=tmp_path).full


def test_an_independent_python_test_remains_affected(tmp_path):
    source(tmp_path, "tools/isolated_test.py", "def test_case(): assert 1 == 1")
    plan = select(["tools/isolated_test.py"], root=tmp_path)
    assert not plan.full and plan.python == ("tools",)
