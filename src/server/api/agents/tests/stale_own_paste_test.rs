use super::*;

const CAPTURES: [&[u8]; 2] = [
    include_bytes!("fixtures/claude_2_1_310_cjk_123cols.pty"),
    include_bytes!("fixtures/claude_2_1_310_cjk_123cols_color.pty"),
];
const PAYLOAD: &str = include_str!("fixtures/stale_bus_cjk_payload.txt");

#[tokio::test]
async fn captured_claude_2_1_310_owns_the_incident_cjk_paste_at_123_columns() {
    // Real Claude Code 2.1.310, 123x48, exact incident payload, bracketed
    // paste without Enter. No model call; captured from isolated PTYs,
    // with and without color (the Bus pane environment enables color).
    let runtime = crate::terminal::TerminalRuntime::test_with_channel_capacity(123, 48, 8).0;
    for stream in CAPTURES {
        runtime.test_process_pty_bytes(b"\x1bc");
        runtime.test_process_pty_bytes(stream);
        let screen = runtime.visible_ansi();
        assert_eq!(
            observe(AgentKind::Claude, &screen, PAYLOAD),
            InputObservation::Pending,
            "body={:?}, screen={screen:?}",
            input(AgentKind::Claude, &screen)
        );
    }
}

#[tokio::test]
async fn captured_cjk_draft_with_incident_subagent_footer_and_status_rows_is_owned() {
    let runtime = crate::terminal::TerminalRuntime::test_with_channel_capacity(123, 48, 8).0;
    let chrome = include_str!("fixtures/claude_subagent_footer.txt")
        .lines()
        .skip(3)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(chrome.contains("← 1 agent"));
    for stream in CAPTURES {
        runtime.test_process_pty_bytes(b"\x1bc");
        runtime.test_process_pty_bytes(stream);
        let screen = runtime.visible_ansi();
        let rows: Vec<_> = screen.lines().collect();
        let bottom = rows.iter().rposition(|line| plain_rule(line)).unwrap();
        let screen = format!("{}\n{chrome}", rows[..=bottom].join("\n"));
        assert_eq!(
            observe(AgentKind::Claude, &screen, PAYLOAD),
            InputObservation::Pending,
            "body={:?}, screen={screen:?}",
            input(AgentKind::Claude, &screen)
        );
    }
}
