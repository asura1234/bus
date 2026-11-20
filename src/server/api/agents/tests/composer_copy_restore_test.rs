//! Red-first tests for unified snapshot -> send -> restore in the shared agent-pane delivery path.
use super::*;

#[derive(Clone, Copy, Debug)]
enum DeliveryTiming {
    Queue,
    Steer,
    Interrupt,
}

fn pane_screen(kind: AgentKind, draft: &str) -> String {
    let body = match kind {
        AgentKind::Claude => {
            format!("────────────────────────\r\n❯ {draft}\r\n────────────────────────")
        }
        AgentKind::Codex => {
            format!("› {draft}\r\n\r\n GPT-6 · /repo\r\n ? for shortcuts")
        }
        AgentKind::Antigravity => {
            format!(
                "────────────────────────\r\n> {draft}\r\n────────────────────────\r\n? for shortcuts"
            )
        }
        AgentKind::Cursor => format!(" > {draft}\r\n / commands · @ files"),
        _ => unreachable!(),
    };
    format!("\x1b[2J\x1b[H{body}")
}

fn paint_pane(app: &App, kind: AgentKind, draft: &str) {
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(pane_screen(kind, draft).as_bytes());
}

fn setup_pane(
    kind: AgentKind,
    timing: DeliveryTiming,
    initial_draft: &str,
) -> (App, Method, tokio::sync::mpsc::Receiver<Bytes>) {
    let (mut app, mut method, writes) = provider_fixture(kind, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
        .attached_terminal_id
        .clone();
    let state = match timing {
        DeliveryTiming::Queue => AgentState::Idle,
        DeliveryTiming::Steer | DeliveryTiming::Interrupt => AgentState::Working,
    };
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_detected_state(Some(kind), state);
    paint_pane(&app, kind, initial_draft);
    if let Method::AgentPromptIfIdle(params) = &mut method {
        params.text = "deliver this prompt".into();
        params.steer = matches!(timing, DeliveryTiming::Steer);
        params.allow_working = matches!(timing, DeliveryTiming::Interrupt);
    }
    (app, method, writes)
}

fn apply_simple_chunk(
    app: &App,
    kind: AgentKind,
    chunk: Bytes,
    draft: &mut String,
    submitted: &mut Vec<String>,
    saw_clear_or_stash: &mut bool,
) {
    let text = std::str::from_utf8(&chunk).unwrap();
    if let Some(pasted) = text
        .strip_prefix("\x1b[200~")
        .and_then(|s| s.strip_suffix("\x1b[201~"))
    {
        draft.push_str(pasted);
        paint_pane(app, kind, draft);
    } else if text == "\r" {
        submitted.push(std::mem::take(draft));
        paint_pane(app, kind, "");
    } else if matches!(text, "\x01" | "\x0b" | "\x15") {
        *saw_clear_or_stash = true;
        if text == "\x0b" || text == "\x15" {
            draft.clear();
            paint_pane(app, kind, "");
        }
    } else if text == "\x13" || text == "\x07" {
        // Simulate a terminal without native Ctrl+S stash / external editor handling so
        // unified copy/send/restore is exercised without holding.
        *saw_clear_or_stash = true;
    }
}

async fn pump_simple_composer(
    app: &App,
    kind: AgentKind,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    draft: &mut String,
    submitted: &mut Vec<String>,
    saw_clear_or_stash: &mut bool,
    response: std::sync::mpsc::Receiver<String>,
) -> String {
    let mut response = tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap());
    loop {
        tokio::select! {
            reply = &mut response => {
                while let Ok(chunk) = writes.try_recv() {
                    apply_simple_chunk(app, kind, chunk, draft, submitted, saw_clear_or_stash);
                }
                return reply.unwrap();
            }
            chunk = next_write(writes) => {
                apply_simple_chunk(app, kind, chunk, draft, submitted, saw_clear_or_stash);
            }
        }
    }
}

#[tokio::test]
async fn shared_delivery_copy_and_restore_round_trip_for_queue_steer_and_interrupt() {
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Antigravity] {
        for timing in [
            DeliveryTiming::Queue,
            DeliveryTiming::Steer,
            DeliveryTiming::Interrupt,
        ] {
            let (mut app, method, mut writes) = setup_pane(kind, timing, "half typed by person");
            let mut draft = "half typed by person".to_owned();
            let mut submitted = Vec::new();
            let mut saw_clear = false;
            let response = start(&mut app, method);
            clock(&app).resume();
            let reply = pump_simple_composer(
                &app,
                kind,
                &mut writes,
                &mut draft,
                &mut submitted,
                &mut saw_clear,
                response,
            )
            .await;

            assert!(
                reply.contains("agent_prompted"),
                "{kind:?}/{timing:?}: delivery must succeed without holding queue: {reply}"
            );
            assert_eq!(
                submitted,
                ["deliver this prompt"],
                "{kind:?}/{timing:?}: delivered message must be submitted cleanly"
            );
            assert_eq!(
                draft, "half typed by person",
                "{kind:?}/{timing:?}: saved copy must be restored into composer after submit"
            );
            assert!(
                writes.try_recv().is_err(),
                "{kind:?}/{timing:?}: restored copy must not receive Enter"
            );
        }
    }
}

#[tokio::test]
async fn shared_delivery_skips_snapshot_and_restore_when_composer_is_empty() {
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Antigravity] {
        for timing in [
            DeliveryTiming::Queue,
            DeliveryTiming::Steer,
            DeliveryTiming::Interrupt,
        ] {
            let (mut app, method, mut writes) = setup_pane(kind, timing, "");
            let mut draft = String::new();
            let mut submitted = Vec::new();
            let mut saw_clear = false;
            let response = start(&mut app, method);
            clock(&app).resume();
            let reply = pump_simple_composer(
                &app,
                kind,
                &mut writes,
                &mut draft,
                &mut submitted,
                &mut saw_clear,
                response,
            )
            .await;

            assert!(
                reply.contains("agent_prompted"),
                "{kind:?}/{timing:?}: empty composer delivery must succeed: {reply}"
            );
            assert_eq!(submitted, ["deliver this prompt"], "{kind:?}/{timing:?}");
            assert!(
                draft.is_empty(),
                "{kind:?}/{timing:?}: empty composer must remain empty"
            );
            assert!(
                !saw_clear,
                "{kind:?}/{timing:?}: empty composer must skip snapshot/clear/restore"
            );
        }
    }
}

#[tokio::test]
async fn shared_delivery_delivers_anyway_when_composer_has_opaque_chip() {
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Antigravity] {
        let (mut app, method, mut writes) = setup_pane(
            kind,
            DeliveryTiming::Interrupt,
            "[Pasted text #1 1500 chars]",
        );
        let mut draft = "[Pasted text #1 1500 chars]".to_owned();
        let mut submitted = Vec::new();
        let mut saw_clear = false;
        let response = start(&mut app, method);
        clock(&app).resume();
        let reply = pump_simple_composer(
            &app,
            kind,
            &mut writes,
            &mut draft,
            &mut submitted,
            &mut saw_clear,
            response,
        )
        .await;

        assert!(
            reply.contains("agent_prompted"),
            "{kind:?}: unrestorable composer must still deliver and never block with composer_busy: {reply}"
        );
        assert!(
            !submitted.is_empty() && submitted[0].contains("deliver this prompt"),
            "{kind:?}: message must be submitted: {submitted:?}"
        );
    }
}
