from pathlib import Path

import pytest

from tools.quality import hot_path


def test_render_hot_paths_avoid_known_expensive_runtime_queries():
    violations = hot_path.find_violations(
        hot_path.source_paths("hot_path"), hot_path.FORBIDDEN_CALLS
    )
    assert not violations, "Render/layout code performs expensive reads:\n" + "\n".join(
        violations
    )


def test_app_and_server_avoid_aggregate_terminal_state():
    violations = hot_path.find_violations(
        hot_path.source_paths("app_server"), hot_path.AGGREGATE_STATE_CALLS
    )
    assert not violations, "App/server code needs narrow accessors:\n" + "\n".join(
        violations
    )


def test_scanner_ignores_non_production_references():
    source = """
// runtime.input_state()
const EXAMPLE: &str = "runtime.input_state()";
  #[cfg(test)]
  mod tests {
    fn aggregate_state_test() { runtime.input_state(); }
}
fn production_after_tests() {}
"""
    code = hot_path.production_code(source)
    assert not hot_path.INPUT_STATE_CALL.search(code)
    assert "fn production_after_tests()" in code
    assert code.count("\n") == source.count("\n")


def test_scanner_checks_production_after_multiple_test_modules():
    source = r"""
#[cfg(test)]
mod tests { const BRACE: char = '\u{7d}'; const TEXT: &str = "}}"; }
#[cfg(test)]
mod more_tests { fn test() { runtime.input_state(); } }
fn render() { TerminalRuntime::input_state; }
"""
    code = hot_path.production_code(source)
    assert len(hot_path.INPUT_STATE_CALL.findall(code)) == 1
    assert code.count("\n") == source.count("\n")


@pytest.mark.parametrize(
    "name", ["input_state", "keyboard_state_ansi", "kitty_keyboard_state_ansi"]
)
def test_scanner_catches_each_aggregate_state_call(name):
    code = hot_path.production_code(f"fn render() {{ runtime.{name}(); }}")
    assert any(pattern.search(code) for pattern, _ in hot_path.AGGREGATE_STATE_CALLS)


def test_scanner_catches_imported_process_query():
    assert hot_path.FORBIDDEN_CALLS[3][0].search("fn render() { foreground_job(pid); }")


def test_mask_preserves_lines_and_ignores_nested_comments_and_literals():
    source = r"""/* outer /* crate::server::X */ crate::terminal::X */
const RAW: &str = br##"crate::client::X\n"##;
const STRING: &str = "escaped \" crate::server::X";
const CHAR: char = '\u{7b}';
fn f<'a>(s: &'a str) { crate::protocol::read(s); }
"""
    code = hot_path.mask_comments_and_literals(source)
    assert len(code) == len(source)
    assert code.count("\n") == source.count("\n")
    assert "crate::server" not in code and "crate::client" not in code
    assert "'a" in code and "crate::protocol::read" in code


def test_globs_deduplicate_files_and_exclude_test_sources(tmp_path, monkeypatch):
    for name in (
        "src/server/rendering/surface/mod.rs",
        "src/server/rendering/surface/draw.rs",
        "src/server/rendering/surface/tests.rs",
        "src/server/rendering/surface/draw_tests.rs",
        "src/server/rendering/surface/tests/draw.rs",
        "src/server/workspaces/agent_view.rs",
    ):
        path = tmp_path / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("fn render() {}", encoding="utf-8")
    monkeypatch.setitem(
        hot_path.SOURCES,
        "hot_path",
        (
            "src/server/rendering/surface/mod.rs",
            "src/server/rendering/surface/**/*.rs",
            "src/server/rendering/surface/draw.rs",
            "src/server/workspaces/agent_view.rs",
        ),
    )
    assert hot_path.source_paths("hot_path", tmp_path) == (
        tmp_path / "src/server/rendering/surface/draw.rs",
        tmp_path / "src/server/rendering/surface/mod.rs",
        tmp_path / "src/server/workspaces/agent_view.rs",
    )


def test_empty_source_group_fails(tmp_path):
    with pytest.raises(ValueError, match="No hot_path Rust sources"):
        hot_path.source_paths("hot_path", tmp_path)


def test_fixture_check_reports_original_line_and_updated_globs(tmp_path, monkeypatch):
    path = tmp_path / "src/server/rendering/surface/draw.rs"
    path.parent.mkdir(parents=True)
    path.write_text(
        "// ignored\nfn draw() { runtime.input_state(); }\n", encoding="utf-8"
    )
    monkeypatch.setitem(
        hot_path.SOURCES, "hot_path", ("src/server/rendering/surface/**/*.rs",)
    )
    monkeypatch.setitem(hot_path.SOURCES, "app_server", ("src/server/**/*.rs",))
    violations = hot_path.check(tmp_path)
    assert len(violations) == 2
    assert all(
        line.startswith("src/server/rendering/surface/draw.rs:2:")
        for line in violations
    )


def test_project_root_is_repository():
    assert hot_path.PROJECT_ROOT == Path(__file__).resolve().parents[2]


def test_moved_render_and_socket_sources_keep_their_checks(tmp_path):
    render_paths = (
        "src/utils/render/widgets.rs",
        "src/utils/render/feedback.rs",
        "src/utils/render/diagnostic.rs",
        "src/utils/text/width.rs",
    )
    socket_path = "src/utils/paths/socket.rs"
    for name in (*render_paths, socket_path):
        path = tmp_path / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("fn draw() { runtime.input_state(); }\n", encoding="utf-8")
    violations = hot_path.check(tmp_path)
    assert len(violations) == 5
    assert {line.split(":", 1)[0] for line in violations} == {
        *render_paths,
        socket_path,
    }
