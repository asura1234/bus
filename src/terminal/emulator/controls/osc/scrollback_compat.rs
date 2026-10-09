use std::borrow::Cow;

pub(super) fn foreground_job_uses_droid_scrollback_compat(
    job: &crate::platform::ForegroundJob,
) -> bool {
    job.processes.iter().any(|process| {
        process.name.eq_ignore_ascii_case("droid")
            || process
                .argv0
                .as_deref()
                .is_some_and(|argv0| argv0.eq_ignore_ascii_case("droid"))
            || process.cmdline.as_deref().is_some_and(|cmdline| {
                cmdline.eq_ignore_ascii_case("droid")
                    || cmdline.starts_with("droid ")
                    || cmdline.to_ascii_lowercase().contains("/droid")
            })
    })
}

pub(in crate::terminal::emulator) fn contains_scrollback_clear_sequence(bytes: &[u8]) -> bool {
    bytes.windows(4).any(|window| window == b"\x1b[3J")
        || bytes.windows(5).any(|window| window == b"\x1b[?3J")
}

pub(super) fn strip_scrollback_clear_sequences<'a>(bytes: &'a [u8]) -> Cow<'a, [u8]> {
    if !contains_scrollback_clear_sequence(bytes) {
        return Cow::Borrowed(bytes);
    }

    let mut filtered = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let remaining = &bytes[index..];
        if remaining.starts_with(b"\x1b[3J") {
            index += 4;
            continue;
        }
        if remaining.starts_with(b"\x1b[?3J") {
            index += 5;
            continue;
        }
        filtered.push(bytes[index]);
        index += 1;
    }

    Cow::Owned(filtered)
}

pub(in crate::terminal::emulator) fn maybe_filter_primary_screen_scrollback_clear<'a>(
    bytes: &'a [u8],
    alternate_screen: bool,
    foreground_job: Option<&crate::platform::ForegroundJob>,
) -> Cow<'a, [u8]> {
    // Droid redraws its primary-screen TUI with CSI 3 J, which erases pane
    // scrollback inside bus. Keep the hack scoped to Droid on the primary
    // screen so normal terminal clear-history behavior still works elsewhere.
    if alternate_screen
        || !contains_scrollback_clear_sequence(bytes)
        || !foreground_job.is_some_and(foreground_job_uses_droid_scrollback_compat)
    {
        return Cow::Borrowed(bytes);
    }

    strip_scrollback_clear_sequences(bytes)
}
