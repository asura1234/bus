//! One local Codex runtime shared by Bus input and the native TUI.
pub(crate) mod launcher;
mod rpc;
mod runtime;
#[cfg(test)]
pub(crate) use runtime::test_support;
pub(crate) use runtime::Runtime;

fn remote_argv(argv: &[String], endpoint: &str) -> Vec<String> {
    let args = &argv[1..];
    let removed: std::collections::HashSet<_> =
        crate::agents::providers::launch::argument_positions(
            crate::agents::providers::ProviderKind::Codex,
            args,
        )
        .into_iter()
        .filter(|&index| args[index] == "--no-daemon")
        .collect();
    std::iter::once(argv[0].clone())
        .chain(["--remote".into(), endpoint.into()])
        .chain(
            args.iter()
                .enumerate()
                .filter(|(index, _)| !removed.contains(index))
                .map(|(_, arg)| arg.clone()),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn codex_native_tui_attaches_to_same_endpoint_without_a_private_second_runtime() {
        let argv = [
            "/bin/codex",
            "--no-daemon",
            "-c",
            "note=\"--no-daemon\"",
            "resume",
            "owned-thread",
            "--",
            "--no-daemon",
        ]
        .map(str::to_owned);
        assert_eq!(
            super::remote_argv(&argv, "unix:///private/runtime/s"),
            [
                "/bin/codex",
                "--remote",
                "unix:///private/runtime/s",
                "-c",
                "note=\"--no-daemon\"",
                "resume",
                "owned-thread",
                "--",
                "--no-daemon"
            ]
            .map(str::to_owned)
        );
    }
}
