//! Real tracing events for scratch TUI acceptance; compiled only with dev-tools.
pub(crate) fn emit(root: &std::path::Path) {
    use std::io::Read;
    let Ok(mut file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("error-pipe-fixture"))
    else {
        return;
    };
    if file.lock().is_err() {
        return;
    }
    let mut events = String::new();
    if file.read_to_string(&mut events).is_err() || file.set_len(0).is_err() {
        return;
    }
    drop(file);
    for event in events.lines() {
        tracing::error!(
            event,
            private_context = "fixture-private-context",
            "fixture-private-error"
        );
    }
}
