from pathlib import Path

import pytest

from tools.quality import import_boundaries as boundaries


def write_source(root, name, source):
    path = root / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(source, encoding="utf-8")


def seed(root):
    for owner in boundaries.DEPENDENCIES:
        write_source(root, f"src/{owner}/mod.rs", """
pub struct Item;
pub struct Client;
pub struct Terminal;
pub struct Server;
pub struct Id;
pub struct AgentKind;
pub struct RoomAgent;
pub enum TerminalEvent { Changed }
pub fn stop() {}
""")


def references(root, name):
    return [ref for ref in boundaries.scan(root)[1] if ref.path.as_posix() == name]


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
    path = Path("src/server/terminals/resume.rs" if owner == "server" else f"src/{owner}/mod.rs")
    for target in boundaries.DEPENDENCIES:
        assert boundaries.allowed(owner, target, path) == (target in targets | {owner, "utils"})


def test_legacy_sources_and_references_have_same_owners():
    for owner in boundaries.DEPENDENCIES:
        assert boundaries.component(owner + "::Item") == owner
        assert boundaries.source_component(Path(f"src/{owner}/nested.rs")) == owner
    # Intentional invalid fixtures: obsolete names receive no approximate owner.
    for obsolete in ("app", "bus", "pane", "detect", "events", "ui", "session"):
        assert boundaries.component(obsolete) is None
        assert boundaries.source_component(Path(f"src/{obsolete}.rs")) is None
    assert not hasattr(boundaries, "LEGACY_ROOTS")
    assert not hasattr(boundaries, "LEGACY_PATHS")


def test_s5_facades_keep_utils_and_server_ownership(tmp_path):
    seed(tmp_path)
    name = "src/protocol/api/client.rs"
    write_source(tmp_path, name, """use crate::protocol::Item;
use crate::server::Server;
use crate::utils::Id;
use crate::utils::Item;
""")
    assert [(r.target, r.forbidden) for r in references(tmp_path, name)] == [
        ("protocol", False), ("server", True), ("utils", False), ("utils", False),
    ]


def test_report_masks_comments_and_literals_but_checks_test_code(tmp_path):
    seed(tmp_path)
    name = "src/client/run.rs"
    write_source(tmp_path, name, '''// crate::terminal::Ignored
const EXAMPLE: &str = r###"crate::server::Ignored"###;
/* crate::server::Ignored /* nested */ */
use crate::protocol::Item;
#[cfg(test)]
mod tests { use crate::terminal::Terminal; }
''')
    refs = references(tmp_path, name)
    assert [(r.target, r.line, r.forbidden) for r in refs] == [
        ("protocol", 4, False), ("terminal", 6, True),
    ]
    assert refs[1].test_only


def test_parser_handles_multiline_and_grouped_imports(tmp_path):
    seed(tmp_path)
    name = "src/client/run.rs"
    write_source(tmp_path, name, """use crate :: protocol :: Item;
use crate::{
    agents::AgentKind, r#terminal::{Terminal, TerminalEvent},
    platform::Item as PlatformItem,
};
fn call() { crate::server::stop(); }
""")
    refs = references(tmp_path, name)
    assert [(r.target, r.line) for r in refs] == [
        ("protocol", 1), ("agents", 3), ("terminal", 3), ("terminal", 3),
        ("platform", 4), ("server", 6),
    ]
    assert [r.target for r in refs if r.forbidden] == ["terminal", "terminal", "server"]


def test_main_and_render_scale_exception_are_narrow(tmp_path):
    seed(tmp_path)
    names = ("src/main.rs", "src/server/tests/render_scale_test.rs",
             "src/server/render_scale.rs", "src/server/tests/other_test.rs")
    for name in names:
        write_source(tmp_path, name, "use crate::client::Client; use crate::terminal::Terminal;")
    _, refs = boundaries.scan(tmp_path)
    assert [r.path.as_posix() for r in refs if r.forbidden] == list(names[2:])


def test_legacy_edges_and_composition_shim(tmp_path):
    seed(tmp_path)
    # Main cannot launder a deleted root through a newly introduced alias.
    write_source(tmp_path, "src/main.rs", "pub use crate::terminal as pane;")
    name = "src/messaging/example.rs"
    write_source(tmp_path, name, "use crate::pane::Terminal; use crate::app::App;")
    assert all(r.target == "unknown" and r.forbidden for r in references(tmp_path, name))
    write_source(tmp_path, "src/compat_paths.rs", "pub use crate::terminal as pane;")
    with pytest.raises(ValueError, match="Unmapped source owner"):
        boundaries.scan(tmp_path)


def test_shared_root_constants_are_not_unknown_components(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/utils/runtime.rs", 'pub const HERDR_ENV_VAR: &str = "HERDR_ENV";\npub const HERDR_ENV_VALUE: &str = "1";')
    write_source(tmp_path, "src/utils/mod.rs", "mod runtime;")
    name = "src/terminal/runtime/mod.rs"
    write_source(tmp_path, name, "fn spawn() { cmd.env(crate::utils::runtime::HERDR_ENV_VAR, crate::utils::runtime::HERDR_ENV_VALUE); }")
    assert [(r.target, r.forbidden) for r in references(tmp_path, name)] == [("utils", False)] * 2


def test_s6_facades_keep_agent_and_viewport_ownership(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/agents/title.rs", "pub fn stripped_terminal_title() {}")
    write_source(tmp_path, "src/agents/mod.rs", "pub mod title;")
    write_source(tmp_path, "src/server/app/mod.rs", "pub struct App;")
    write_source(tmp_path, "src/server/mod.rs", "pub mod app;")
    name = "src/client/run.rs"
    write_source(tmp_path, name, """use crate::agents::title;
use crate::agents::title::stripped_terminal_title;
use crate::server::app::App;
""")
    assert [(r.target, r.forbidden) for r in references(tmp_path, name)] == [
        ("agents", False), ("agents", False), ("server", True),
    ]


def test_default_reports_and_enforce_fails(tmp_path, capsys):
    seed(tmp_path)
    write_source(tmp_path, "src/protocol/example.rs", "use crate::server::Server;\n")
    args = ["--root", str(tmp_path)]
    assert boundaries.main(args) == 0
    report = capsys.readouterr().out
    assert "1 forbidden edges, 1 forbidden references" in report
    assert "src/protocol/example.rs:1: protocol -> server" in report
    assert boundaries.main([*args, "--enforce"]) == 1


def test_clean_tree_passes_enforcement(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/messaging/model/mod.rs", "use crate::utils::Id; use crate::agents::AgentKind;")
    assert boundaries.main(["--root", str(tmp_path), "--enforce"]) == 0


def test_unknown_reference_is_reported_and_fails_enforcement(tmp_path, capsys):
    seed(tmp_path)
    write_source(tmp_path, "src/client/example.rs", "use crate::unmapped::Thing;")
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
    assert not (boundaries.PROJECT_ROOT / "src/compat_paths.rs").exists()
    assert boundaries.main([]) == 0
    report = capsys.readouterr().out
    assert "Import boundaries:" in report
    assert "0 forbidden edges, 0 forbidden references" in report


def test_s8_input_lease_facade_keeps_client_ownership(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/client/pane_input/lease.rs", "pub struct InputLeaseKey; pub struct InputLeaseTable; pub struct RepeatPlan;")
    write_source(tmp_path, "src/client/mod.rs", "pub mod pane_input { pub mod lease; }")
    name = "src/server/example.rs"
    write_source(tmp_path, name, """use crate::client::pane_input::lease::InputLeaseKey;
use crate::client::pane_input::lease::InputLeaseTable;
use crate::client::pane_input::lease::RepeatPlan;
use crate::protocol::Item;
""")
    assert [(r.target, r.forbidden) for r in references(tmp_path, name)] == [
        ("client", True), ("client", True), ("client", True), ("protocol", False),
    ]


def test_s11_socket_preparation_is_server_policy(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/platform/ipc.rs", "pub fn prepare_socket_path() {}")
    write_source(tmp_path, "src/platform/mod.rs", "pub mod ipc;")
    write_source(tmp_path, "src/server/socket_paths.rs", "fn prepare() { crate::platform::ipc::prepare_socket_path(); }")
    write_source(tmp_path, "src/utils/socket_paths.rs", "pub fn path() {}")
    write_source(tmp_path, "src/utils/mod.rs", "pub mod socket_paths;")
    name = "src/client/example.rs"
    write_source(tmp_path, name, "use crate::server::socket_paths; use crate::utils::socket_paths;")
    assert [(r.target, r.forbidden) for r in references(tmp_path, name)] == [("server", True), ("utils", False)]
    assert all(not r.forbidden for r in references(tmp_path, "src/server/socket_paths.rs"))


@pytest.mark.parametrize("owner", tuple(boundaries.DEPENDENCIES))
@pytest.mark.parametrize("target", tuple(boundaries.DEPENDENCIES))
def test_each_component_edge_is_checked_from_real_sources(tmp_path, owner, target):
    seed(tmp_path)
    name = "src/server/terminals/resume.rs" if owner == "server" else f"src/{owner}/example.rs"
    write_source(tmp_path, name, f"use crate::{target}::Item;")
    refs = references(tmp_path, name)
    assert len(refs) == 1
    expected = target in {owner, "utils"} | set(boundaries.DEPENDENCIES[owner])
    assert refs[0].forbidden == (not expected)


@pytest.mark.parametrize("directory", ("vt", "pty", "emulator"))
@pytest.mark.parametrize("test", (False, True))
def test_terminal_inner_boundaries_apply_to_all_cfg_scopes(tmp_path, directory, test):
    seed(tmp_path)
    name = f"src/terminal/{directory}/example.rs"
    text = "use crate::agents::AgentKind; use crate::protocol::Item;"
    write_source(tmp_path, name, f"#[cfg(test)] mod tests {{ {text} }}" if test else text)
    assert [(r.target, r.forbidden) for r in references(tmp_path, name)] == [("agents", True), ("protocol", False)]


@pytest.mark.parametrize("name,forbidden", (
    ("src/server/terminals/resume.rs", False),
    ("src/server/terminals/resume_helper.rs", True),
    ("src/server/terminals/resume/tests/example_test.rs", True),
    ("src/server/api/resume.rs", True),
))
def test_server_messaging_exception_is_one_physical_file(tmp_path, name, forbidden):
    seed(tmp_path)
    write_source(tmp_path, name, "use crate::messaging::RoomAgent;")
    assert references(tmp_path, name)[0].forbidden == forbidden


@pytest.mark.parametrize("target", ("messaging", "server", "client", "cli"))
def test_provider_code_cannot_consume_product_components(tmp_path, target):
    seed(tmp_path)
    name = "src/agents/providers/example.rs"
    write_source(tmp_path, name, f"use crate::{target}::Item;")
    assert references(tmp_path, name)[0].forbidden


def test_unknown_member_of_known_component_fails_closed(tmp_path):
    seed(tmp_path)
    name = "src/client/example.rs"
    write_source(tmp_path, name, "use crate::protocol::does_not_exist::Item;")
    ref = references(tmp_path, name)[0]
    assert ref.target == "unknown" and ref.forbidden


def test_alias_usage_and_multihop_reexports_cannot_hide_an_edge(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/protocol/bridge.rs", "pub use crate::terminal::Terminal as Hidden;")
    write_source(tmp_path, "src/protocol/mod.rs", "mod bridge; pub use bridge::Hidden as Renamed;")
    name = "src/client/example.rs"
    write_source(tmp_path, name, """use crate::protocol::{self as p, Renamed as T};
fn f() { let _: T; p::Renamed::new(); }
""")
    refs = references(tmp_path, name)
    assert not any(r.target == "unknown" for r in refs)
    assert any(r.line == 2 and r.target == "terminal" and r.forbidden for r in refs)
    assert all(r.forbidden for r in refs if r.target == "terminal")


def test_relative_paths_resolve_inline_modules_and_multiple_super_segments(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/client/mod.rs", """pub use crate::terminal::Terminal as Hidden;
mod nested {
    mod deep {
        use super::super::Hidden as T;
        fn f() { self::T::new(); super::super::Hidden::new(); }
    }
}
""")
    refs = references(tmp_path, "src/client/mod.rs")
    assert any(r.line == 4 and r.target == "terminal" and r.forbidden for r in refs)
    assert any(r.line == 5 and r.target == "terminal" and r.forbidden for r in refs)


def test_local_use_scopes_do_not_leak_into_sibling_functions(tmp_path):
    seed(tmp_path)
    name = "src/client/example.rs"
    write_source(tmp_path, name, """fn first() {
    use crate::terminal::Terminal as T;
    T::new();
}
fn second() {
    use crate::protocol::Item as T;
    T::new();
}
""")
    refs = references(tmp_path, name)
    assert [(r.line, r.target) for r in refs] == [(2, "terminal"), (3, "terminal"), (6, "protocol"), (7, "protocol")]


def test_glob_reexports_follow_hidden_targets(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/protocol/bridge.rs", "pub use crate::terminal::Terminal as Hidden;")
    write_source(tmp_path, "src/protocol/mod.rs", "mod bridge; pub use bridge::*;")
    name = "src/client/example.rs"
    write_source(tmp_path, name, "use crate::protocol::*;\nfn f() { protocol::Hidden::new(); }")
    assert any(r.target == "terminal" and r.forbidden for r in references(tmp_path, name))


def test_reexport_cycles_fail_closed(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/protocol/mod.rs", "pub use self::B as A; pub use self::A as B;")
    name = "src/client/example.rs"
    write_source(tmp_path, name, "use crate::protocol::A;")
    assert any(r.target == "unknown" for r in references(tmp_path, name))


def test_external_aliases_and_associated_items_do_not_create_unknown_edges(tmp_path):
    seed(tmp_path)
    name = "src/client/example.rs"
    write_source(tmp_path, name, """use std::{io as IO, collections::HashMap as Map};
use crate::protocol::Item as P;
fn f() { IO::Error::other(1); Map::new(); P::new(); }
""")
    assert all(r.target == "protocol" and not r.forbidden for r in references(tmp_path, name))


def test_literal_path_modules_and_include_contexts_use_physical_owners(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/client/mod.rs", '''#[cfg(test)]
#[path = "tests/renamed_test.rs"] mod suite;
mod nested { include!("tests/included_test.rs"); }
''')
    write_source(tmp_path, "src/client/tests/renamed_test.rs", "use super::nested::Hidden;")
    write_source(tmp_path, "src/client/tests/included_test.rs", "pub use crate::terminal::Terminal as Hidden;")
    refs = references(tmp_path, "src/client/tests/renamed_test.rs")
    assert any(r.target == "terminal" and r.forbidden for r in refs)


@pytest.mark.parametrize("expression", ('include!(concat!(env!("OUT_DIR"), "/generated.rs"));', 'mod missing;'))
def test_unresolvable_source_edges_require_explicit_audit(tmp_path, expression):
    write_source(tmp_path, "src/client/mod.rs", expression)
    with pytest.raises(ValueError, match="computed include|Unresolved owned"):
        boundaries.scan(tmp_path)


def test_unreviewed_macro_definitions_are_visible_audit_failures(tmp_path):
    seed(tmp_path)
    name = "src/client/example.rs"
    write_source(tmp_path, name, "macro_rules! hidden { ($root:ident) => { $root::stop(); }; }")
    assert any(r.forbidden and "macro-generated paths" in r.reason for r in references(tmp_path, name))


def test_audited_id_generator_registers_only_explicit_symbol_arguments(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/messaging/model/types.rs", """macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub(crate) struct $name(pub(crate) u64);
    };
}
id_type!(RoomId);
""")
    name = "src/client/example.rs"
    write_source(tmp_path, name, "use crate::messaging::model::types::RoomId;")
    # Fallback source indexing checks owned files even in a fixture without main.
    assert references(tmp_path, name)[0].target == "messaging"
    assert not any(r.reason for r in boundaries.scan(tmp_path)[1])


def test_render_scale_exception_needs_a_test_context():
    path = Path("src/server/tests/render_scale_test.rs")
    assert not boundaries.allowed("server", "client", path)
    assert boundaries.allowed("server", "client", path, test_only=True)
    assert not boundaries.allowed("server", "client", Path("src/server/tests/other_test.rs"), test_only=True)


def test_recursive_globs_terminate_and_do_not_capture_external_paths(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/protocol/mod.rs", "pub use self::*; pub use crate::terminal::Terminal as Hidden;")
    name = "src/client/example.rs"
    write_source(tmp_path, name, "use crate::protocol::*;\nfn f() { std::io::sink(); Hidden::new(); }")
    refs = references(tmp_path, name)
    assert any(r.target == "terminal" and r.forbidden for r in refs)
    assert not any(r.target == "unknown" for r in refs)


def test_recursive_aliases_terminate_even_when_each_step_adds_segments(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/protocol/mod.rs", "pub use Alias::Item as Alias;")
    name = "src/client/example.rs"
    write_source(tmp_path, name, "use crate::protocol::Alias;")
    assert any(r.target == "unknown" for r in references(tmp_path, name))


@pytest.mark.parametrize("expression", ("crate::protocol::secret", "crate::protocol::stop::Missing"))
def test_local_functions_and_impl_methods_are_not_namespace_members(tmp_path, expression):
    seed(tmp_path)
    write_source(tmp_path, "src/protocol/mod.rs", "pub struct Item; impl Item { fn secret() {} } pub fn stop() { fn secret() {} }")
    name = "src/client/example.rs"
    write_source(tmp_path, name, f"fn f() {{ {expression}(); }}")
    assert references(tmp_path, name)[0].target == "unknown"


def test_path_declaration_itself_cannot_hide_a_cross_component_edge(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/server/foreign.rs", "pub struct Item;")
    write_source(tmp_path, "src/client/mod.rs", '#[path = "../server/foreign.rs"] mod hidden;')
    assert any(r.target == "server" and r.forbidden for r in references(tmp_path, "src/client/mod.rs"))


def test_changed_audited_macro_requires_a_new_audit(tmp_path):
    seed(tmp_path)
    name = "src/messaging/model/types.rs"
    write_source(tmp_path, name, "macro_rules! id_type { ($name:ident) => { pub use server as $name; }; }")
    assert any(r.forbidden and r.reason for r in references(tmp_path, name))


def test_local_module_names_take_priority_over_component_root_names(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/protocol/api/mod.rs", "mod agents; mod server; pub use agents::*; pub use server::*;")
    write_source(tmp_path, "src/protocol/api/agents.rs", "pub struct AgentParams;")
    write_source(tmp_path, "src/protocol/api/server.rs", "pub struct ServerParams;")
    assert all(r.target == "protocol" and not r.forbidden for r in references(tmp_path, "src/protocol/api/mod.rs"))


def test_modules_and_value_reexports_can_have_the_same_spelling(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/protocol/parse.rs", "pub fn parse() {} pub struct Params;")
    write_source(tmp_path, "src/protocol/mod.rs", "mod parse; pub use parse::{parse, Params};")
    name = "src/client/example.rs"
    write_source(tmp_path, name, "use crate::protocol::{Params, parse::Params as P}; fn f() { crate::protocol::parse(); }")
    assert all(r.target == "protocol" and not r.forbidden for r in references(tmp_path, name))


def test_const_functions_and_external_ffi_declarations_are_registered(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/protocol/mod.rs", 'pub const fn fact() -> bool { true } unsafe extern "C" { fn foreign(); }')
    name = "src/client/example.rs"
    write_source(tmp_path, name, "use crate::protocol::{fact, foreign};")
    assert all(r.target == "protocol" and not r.forbidden for r in references(tmp_path, name))


def test_cfg_glob_alternatives_do_not_report_absent_members_as_unknown(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/protocol/mod.rs", "mod unix; mod windows; #[cfg(unix)] pub use unix::*; #[cfg(windows)] pub use windows::*;")
    write_source(tmp_path, "src/protocol/unix.rs", "pub fn unix_only() {}")
    write_source(tmp_path, "src/protocol/windows.rs", "pub fn windows_only() {}")
    name = "src/client/example.rs"
    write_source(tmp_path, name, "#[cfg(windows)] use crate::protocol::windows_only;")
    assert all(r.target == "protocol" and not r.forbidden for r in references(tmp_path, name))


def test_composition_can_use_its_own_items_but_unknown_roots_still_fail(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/main.rs", "fn own() {} #[cfg(test)] mod tests { use super::*; } use crate::missing::Item;")
    refs = references(tmp_path, "src/main.rs")
    assert all(not r.forbidden for r in refs if r.target == "main")
    assert any(r.forbidden and r.target == "unknown" for r in refs)


def test_unqualified_imports_require_a_known_module_or_external_dependency(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "Cargo.toml", '[dependencies]\nrenamed-package = { package="original", version="1" }\n[target.\'cfg(windows)\'.dependencies]\nwin-helper = "1"\n')
    name = "src/client/example.rs"
    write_source(tmp_path, name, "use renamed_package::Type as T; use win_helper::Type as W; use unknown::Thing;")
    refs = references(tmp_path, name)
    assert len(refs) == 1 and refs[0].target == "unknown"


def test_unqualified_known_module_missing_member_fails_closed(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/client/mod.rs", "mod child; fn f() { child::missing(); }")
    write_source(tmp_path, "src/client/child.rs", "pub fn present() {}")
    assert references(tmp_path, "src/client/mod.rs")[0].target == "unknown"


def test_static_lifetimes_do_not_declare_external_crate_names(tmp_path):
    seed(tmp_path)
    name = "src/client/example.rs"
    write_source(tmp_path, name, "fn lock() -> &'static std::sync::Mutex<()> { std::sync::Mutex::new(()) }")
    assert references(tmp_path, name) == []


def test_generated_ffi_union_names_resolve_as_real_declarations(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/protocol/ffi.rs", "pub union Value { pub raw: u64 }")
    name = "src/client/example.rs"
    write_source(tmp_path, name, "use crate::protocol::ffi::Value; fn f() { Value::default(); }")
    assert all(r.target == "protocol" and not r.forbidden for r in references(tmp_path, name))


def test_test_identity_glue_keeps_the_imported_symbols_physical_owners(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/messaging/mod.rs", '#[cfg(test)] mod provider_glue { include!("../agents/tests/owned_test.rs"); }')
    name = "src/agents/tests/owned_test.rs"
    write_source(tmp_path, name, "use crate::agents::Item; #[cfg(test)] mod tests { use super::*; fn f() { Item::new(); } }")
    assert all(r.target == "agents" and not r.forbidden for r in references(tmp_path, name))


def test_cfg_test_module_wiring_is_not_a_test_import_exception(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/protocol/mod.rs", 'pub struct Item; #[cfg(test)] #[path = "../client/tests/owned_test.rs"] mod tests;')
    name = "src/client/tests/owned_test.rs"
    write_source(tmp_path, name, "use crate::protocol::Item; use crate::server::Server;")
    assert [(r.target, r.forbidden) for r in references(tmp_path, name)] == [("protocol", False), ("server", True)]
    assert references(tmp_path, "src/protocol/mod.rs") == []


def test_external_exports_cannot_hide_the_owned_facade_edge(tmp_path):
    seed(tmp_path)
    write_source(tmp_path, "src/client/mod.rs", "pub use std::io::Error as External;")
    name = "src/server/example.rs"
    write_source(tmp_path, name, "use crate::client::External;")
    assert any(r.target == "client" and r.forbidden for r in references(tmp_path, name))
