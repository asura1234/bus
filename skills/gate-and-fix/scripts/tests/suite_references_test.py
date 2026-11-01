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
    source(tmp_path, "tools/work_test.py", 'from pathlib import Path\nROOT = Path(__file__).resolve().parents[1]\nSCRIPT = ROOT / "skills" / "new/scripts/work.py"\n')
    assert select(["skills/new/scripts/work.py"], root=tmp_path).full


def test_an_independent_python_test_remains_affected(tmp_path):
    source(tmp_path, "tools/isolated_test.py", "def test_case(): assert 1 == 1")
    plan = select(["tools/isolated_test.py"], root=tmp_path)
    assert not plan.full and plan.python == ("tools",)


def test_same_rust_basename_in_another_component_is_not_a_consumer(tmp_path):
    for component in ('agents', 'client'):
        source(tmp_path, f'src/{component}/mod.rs', '#[path="tests/helper_test.rs"] mod helper;')
        source(tmp_path, f'src/{component}/tests/helper_test.rs', '#[test] fn case() {}')
    plan = select(['src/agents/tests/helper_test.rs'], root=tmp_path)
    assert not plan.full and plan.rust_filter == '(binary(bus) & test(/^agents::/))'


def test_tmp_fixture_directory_literal_is_not_a_repository_reference(tmp_path):
    source(tmp_path, 'src/agents/mod.rs', '#[path="tests/local_test.rs"] mod local;')
    source(tmp_path, 'src/agents/tests/local_test.rs', '#[test] fn case() {}')
    source(tmp_path, 'tools/fixture_test.py', 'def test_fixture(tmp_path):\n    source = tmp_path / "src"\n')
    assert not select(['src/agents/tests/local_test.rs'], root=tmp_path).full


def test_bare_python_import_resolves_in_its_source_directory(tmp_path):
    for component in ('tools', 'packaging'):
        source(tmp_path, f'{component}/worker.py', 'def run(): return 1')
        source(tmp_path, f'{component}/worker_test.py', 'import worker\n')
    plan = select(['tools/worker.py'], root=tmp_path)
    assert not plan.full and plan.python == ('tools',)


def test_bundle_rebuild_watch_does_not_embed_skill_test_sources(tmp_path):
    source(tmp_path, 'build.rs', 'fn main() { let skills = root.join("skills"); collect(&skills, &skills, &mut files); println!("cargo:rerun-if-changed=skills"); }')
    source(tmp_path, 'skills/example/scripts/tests/local_test.py', 'def test_case(): pass\n')
    source(tmp_path, 'skills/example/scripts/runtime.py', 'def run(): return 1\n')
    assert not select(['skills/example/scripts/tests/local_test.py'], root=tmp_path).full
    assert select(['skills/example/scripts/runtime.py'], root=tmp_path).full


def test_unrelated_rebuild_watch_is_not_a_runtime_dependency(tmp_path):
    source(tmp_path, 'build.rs', 'fn main() { println!("cargo:rerun-if-changed=tools"); }')
    source(tmp_path, 'tools/runtime.py', 'def run(): return 1\n')
    source(tmp_path, 'tools/runtime_test.py', 'from .runtime import run\n')
    assert not select(['tools/runtime.py'], root=tmp_path).full


def test_bundle_directory_symlink_tracks_production_but_not_test_sources(tmp_path):
    source(tmp_path, 'build.rs', 'fn main() { let skills = root.join("skills"); collect(&skills, &skills, &mut files); }')
    source(tmp_path, 'scripts/runtime.py', 'def run(): return 1\n')
    source(tmp_path, 'scripts/runtime_test.py', 'import runtime\n')
    alias = tmp_path / 'skills/shared/scripts'
    alias.parent.mkdir(parents=True)
    alias.symlink_to(tmp_path / 'scripts', target_is_directory=True)
    assert select(['scripts/runtime.py'], root=tmp_path).full
    assert not select(['scripts/runtime_test.py'], root=tmp_path).full


def test_python_helper_import_carries_the_cli_prerequisite(tmp_path):
    source(tmp_path, 'tools/helper.py', 'import os\nbinary = os.environ["BUS_TEST_BINARY"]\n')
    source(tmp_path, 'tools/case_test.py', 'from .helper import binary\n')
    plan = select(['tools/case_test.py'], root=tmp_path)
    assert not plan.full and plan.binary


def test_explicit_repo_sys_path_resolves_a_cross_root_import(tmp_path):
    source(tmp_path, 'tools/helper.py', 'def run(): return 1\n')
    source(tmp_path, 'packaging/case_test.py', 'import sys\nfrom pathlib import Path\nROOT=Path(__file__).resolve().parents[1]\nsys.path.insert(0, str(ROOT / "tools"))\nfrom helper import run\n')
    assert select(['tools/helper.py'], root=tmp_path).full
