//! The release variant carries no driver code: the bytes of the built `bus`
//! hold none of the driver's strings or symbols. The dev-tools lane runs the
//! same scan on the dev variant, where every marker must be found, so the
//! scan itself is proven to see driver code when it is there.

/// One string literal of the fake provider, the driver's reference text
/// (compiled in as its help), the driver module's symbol path, the
/// screenshot mirror's window title, the bundled screenshot font, the effect
/// recorder's variable, and the room UI's dev-only `ui hits` module.
const DRIVER_MARKERS: [&str; 7] = [
    "fake reply: ",
    "drive a scratch Bus through its real terminal",
    "8devtools3tui",
    "bus-tui-mirror-",
    "DejaVu Sans Mono",
    "BUS_TUI_EFFECTS",
    "7ui_hits",
];

fn markers_in_built_binary() -> Vec<&'static str> {
    let binary = std::fs::read(env!("CARGO_BIN_EXE_bus")).expect("read the built bus");
    DRIVER_MARKERS
        .into_iter()
        .filter(|marker| {
            regex::bytes::Regex::new(&regex::escape(marker))
                .unwrap()
                .is_match(&binary)
        })
        .collect()
}

#[cfg(not(feature = "dev-tools"))]
#[test]
fn a_build_without_dev_tools_contains_no_driver_code() {
    assert_eq!(markers_in_built_binary(), Vec::<&str>::new());
}

#[cfg(feature = "dev-tools")]
#[test]
fn the_dev_variant_contains_the_driver_the_scan_looks_for() {
    assert_eq!(markers_in_built_binary(), DRIVER_MARKERS.to_vec());
}
