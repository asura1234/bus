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

#[cfg(test)]
mod tests {
    use tracing_subscriber::prelude::*;
    #[test]
    fn error_pipe_fixture_emits_each_queued_error_once_through_the_real_subscriber() {
        let root = crate::utils::test_temp::unique_temp_path("bus-error-pipe-emission");
        crate::platform::fs::private_dir(&root).unwrap();
        std::fs::write(
            root.join("error-pipe-fixture"),
            "bus.dev.test\nbus.dev.test\n",
        )
        .unwrap();
        let log = root.join("logs");
        let mut runtime = crate::utils::error_pipe::Runtime::default();
        runtime.configure(&log, Some("lead"));
        let subscriber = tracing_subscriber::registry().with(
            crate::utils::error_pipe::CaptureLayer::new(log.join("server.log")),
        );
        tracing::subscriber::with_default(subscriber, || {
            super::emit(&root);
            super::emit(&root);
        });
        assert_eq!(crate::utils::error_pipe::drain(&log).unwrap().len(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
