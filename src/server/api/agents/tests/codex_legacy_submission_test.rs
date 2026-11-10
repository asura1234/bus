//! A PTY receipt does not mean Codex consumed the prompt.
use super::owned_draft_cleanup_tests::write_or_done;
use super::*;

async fn ignored_enter(developer_edit: bool) {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let expiry = confirmation_expiry(&method);
    let native_clock = clock(&app);
    let paste_receipt = native_clock.hold_next_write_completion();
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes).await.as_ref(),
        b"\x1b[200~from bus\x1b[201~"
    );
    redraw(&app, AgentKind::Codex, "from bus");
    let enter_receipt = native_clock.hold_next_write_completion();
    paste_receipt.send(()).unwrap();
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
    // Codex's picker can consume Enter while leaving the composer intact.
    // The writer acknowledges it, but no model turn has accepted this text.
    enter_receipt.send(()).unwrap();
    native_clock.wait_for_enters(1);
    if developer_edit {
        redraw(&app, AgentKind::Codex, "from bus + developer draft");
    }
    native_clock.advance(expiry);
    let mut response = (
        tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap()),
        None,
    );
    let mut clears = 0;
    let reply = loop {
        match write_or_done(&mut writes, &mut response).await {
            Ok(key) => {
                assert!(
                    !developer_edit,
                    "cleanup edited the developer's draft: {key:?}"
                );
                assert!(
                    matches!(key.as_ref(), b"\x01" | b"\x0b" | b"\x15"),
                    "never replay Enter or paste: {key:?}"
                );
                clears += 1;
                if key.as_ref() == b"\x0b" {
                    redraw(&app, AgentKind::Codex, "");
                }
            }
            Err(reply) => break reply,
        }
    };
    assert!(
        !reply.contains("agent_prompted"),
        "a flushed Enter is not consumption: {reply}"
    );
    assert!(
        !reply.contains("agent_prompt_not_shown"),
        "Enter may already have run: {reply}"
    );
    if developer_edit {
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        assert!(app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .visible_ansi()
            .contains("from bus + developer draft"));
        assert_eq!(clears, 0);
    } else {
        assert!(clears > 0, "withdraw the exact stranded Bus text");
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        assert!(!app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .visible_ansi()
            .contains("from bus"));
    }
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn codex_legacy_ignored_enter_withdraws_owned_text() {
    ignored_enter(false).await;
}

#[tokio::test]
async fn codex_legacy_ignored_enter_preserves_developer_edit() {
    ignored_enter(true).await;
}
