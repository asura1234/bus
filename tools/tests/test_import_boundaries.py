import pytest

from tools.quality import import_boundaries as boundaries


def write_source(root, name, source):
    path = root / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(source, encoding="utf-8")


@pytest.mark.parametrize(
    "owner,targets",
    [
        ("utils", set()),
        ("platform", set()),
        ("protocol", {"platform"}),
        ("agents", {"platform"}),
        ("terminal", {"agents", "protocol", "platform"}),
        ("messaging", {"agents", "protocol", "platform"}),
        ("server", {"terminal", "messaging", "agents", "protocol", "platform"}),
        ("client", {"messaging", "agents", "protocol", "platform"}),
        ("cli", {"client", "messaging", "protocol", "platform"}),
    ],
)
def test_component_graph_matches_architecture(owner, targets):
    path = boundaries.Path(f"src/{owner}/mod.rs")
    for target in boundaries.DEPENDENCIES:
        assert boundaries.allowed(owner, target, path) == (
            target in targets | {owner, "utils"}
        )


def test_legacy_sources_and_references_have_same_owners():
    for module, owner in boundaries.LEGACY_ROOTS.items():
        assert boundaries.component(module) == owner
        assert boundaries.source_component(boundaries.Path(f"src/{module}.rs")) == owner
    for module, owner in boundaries.LEGACY_PATHS.items():
        path = boundaries.Path("src", *module.split("::")).with_suffix(".rs")
        assert boundaries.source_component(path) == owner
        assert boundaries.component(module + "::Item") == owner


def test_report_masks_comments_and_literals_but_checks_test_code(tmp_path):
    write_source(
        tmp_path,
        "src/client/run.rs",
        """// crate::terminal::Ignored
const EXAMPLE: &str = r#"crate::server::Ignored"#;
/* crate::server::Ignored /* nested */ */
use crate::protocol::wire::ClientMessage;
#[cfg(test)]
mod tests { use crate::terminal::TerminalState; }
""",
    )
    checked, refs = boundaries.scan(tmp_path)
    assert checked == 1
    assert [(ref.target, ref.line, ref.forbidden) for ref in refs] == [
        ("protocol", 4, False),
        ("terminal", 6, True),
    ]


def test_parser_handles_multiline_and_grouped_imports(tmp_path):
    write_source(
        tmp_path,
        "src/client/mod.rs",
        """use crate :: protocol :: wire::Message;
use crate::{
    agents::Agent, r#terminal::{Terminal, Runtime},
    platform::Thing,
};
fn call() { crate::server::stop(); }
""",
    )
    _, refs = boundaries.scan(tmp_path)
    assert [(ref.target, ref.line) for ref in refs] == [
        ("protocol", 1),
        ("agents", 3),
        ("terminal", 3),
        ("platform", 4),
        ("server", 6),
    ]
    assert [ref.target for ref in refs if ref.forbidden] == ["terminal", "server"]


def test_main_and_render_scale_exception_are_narrow(tmp_path):
    for name in (
        "src/main.rs",
        "src/server/tests/render_scale.rs",
        "src/server/render_scale.rs",
        "src/server/tests/other.rs",
    ):
        write_source(
            tmp_path, name, "use crate::client::Client; use crate::terminal::Terminal;"
        )
    _, refs = boundaries.scan(tmp_path)
    assert [ref.path.as_posix() for ref in refs if ref.forbidden] == [
        "src/server/render_scale.rs",
        "src/server/tests/other.rs",
    ]


def test_legacy_edges_and_composition_shim(tmp_path):
    write_source(
        tmp_path,
        "src/bus/resume_launch.rs",
        "use crate::pane::Runtime; use crate::app::App;",
    )
    write_source(
        tmp_path, "src/compat_paths.rs", "pub(crate) use crate::terminal as pane;"
    )
    checked, refs = boundaries.scan(tmp_path)
    assert checked == 1
    assert [(ref.owner, ref.target, ref.forbidden) for ref in refs] == [
        ("messaging", "terminal", True),
        ("messaging", "server", True),
    ]


def test_shared_root_constants_are_not_unknown_components(tmp_path):
    write_source(
        tmp_path,
        "src/pane.rs",
        "fn spawn() { cmd.env(crate::HERDR_ENV_VAR, crate::HERDR_ENV_VALUE); }",
    )
    _, refs = boundaries.scan(tmp_path)
    assert [(ref.target, ref.forbidden) for ref in refs] == [("utils", False)] * 2


def test_default_reports_and_enforce_fails(tmp_path, capsys):
    write_source(tmp_path, "src/protocol/mod.rs", "use crate::server::Server;\n")
    args = ["--root", str(tmp_path)]
    assert boundaries.main(args) == 0
    report = capsys.readouterr().out
    assert "1 forbidden edges, 1 forbidden references" in report
    assert "src/protocol/mod.rs:1: protocol -> server" in report
    assert boundaries.main([*args, "--enforce"]) == 1


def test_clean_tree_passes_enforcement(tmp_path):
    write_source(
        tmp_path,
        "src/messaging/model/mod.rs",
        "use crate::utils::Id; use crate::agents::Agent;",
    )
    assert boundaries.main(["--root", str(tmp_path), "--enforce"]) == 0


def test_unknown_reference_is_reported_and_fails_enforcement(tmp_path, capsys):
    write_source(tmp_path, "src/client/mod.rs", "use crate::unmapped::Thing;")
    assert boundaries.main(["--root", str(tmp_path), "--enforce"]) == 1
    assert "client -> unknown (crate::unmapped::Thing)" in capsys.readouterr().out


def test_missing_tree_or_unmapped_source_fails_closed(tmp_path):
    with pytest.raises(ValueError, match="No Rust sources"):
        boundaries.scan(tmp_path)
    write_source(tmp_path, "src/new_owner/mod.rs", "")
    with pytest.raises(ValueError, match="Unmapped source owner"):
        boundaries.scan(tmp_path)
    with pytest.raises(SystemExit) as error:
        boundaries.main(["--root", str(tmp_path)])
    assert error.value.code == 2


def test_repository_can_be_scanned_in_report_mode(capsys):
    assert boundaries.main([]) == 0
    assert "Import boundaries:" in capsys.readouterr().out
