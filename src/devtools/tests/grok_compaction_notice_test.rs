/// The shell fake from `write_fake_agent` is the herdr `agent.start` harness.
/// It has no Bus room, so it cannot post this notice, and a shell script
/// cannot satisfy the Grok composer. This test reads the pane the server
/// uses, then the real matcher, poll, and limit notice.
const OBSERVED_GROK_COMPLETION: &str = "Context compacted: 209.9k -> 37.8k tokens (131.2s)";

fn pane_shows_compaction_banner(screen: &str) -> bool {
    let (runtime, _writes) = crate::terminal::TerminalRuntime::test_with_channel_capacity(80, 8, 4);
    runtime.test_process_pty_bytes(screen.as_bytes());
    let (grid, _) = crate::terminal::driver::observe(&runtime).expect("screen read");
    crate::agents::compaction_banner::visible(Some("grok"), &grid.text())
}

#[tokio::test]
async fn grok_observed_completion_line_reaches_the_limit_notice() {
    let shown = format!("{OBSERVED_GROK_COMPLETION}\r\n");
    let open = pane_shows_compaction_banner(&shown);
    let idle = pane_shows_compaction_banner("ready\r\n");
    let (count, posted) = crate::messaging::coordinator::poll_observed_grok_banner(open, idle);
    assert_eq!(
        count, 5,
        "five episodes of the observed completion line reach the default limit; pane banner {open}, idle {idle}"
    );
    assert_eq!(posted.len(), 1, "the limit notice posts once: {posted:?}");
    assert_eq!(
        posted[0],
        "g1 reached 5 compactions; get a handover note and replace it."
    );
}
