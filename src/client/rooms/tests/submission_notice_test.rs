use super::*;

const DETAIL: &str = "The draft was left because it was edited. Inspect the transcript and composer before resending.";

fn submitting() -> (BusUi, RoomId, AgentId, RequestId) {
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                ..Default::default()
            },
        )
        .unwrap();
    snapshot.state.confirm_hook_setup(agent).unwrap();
    snapshot
        .state
        .observe_status(agent, RuntimeStatus::Idle, 1000)
        .unwrap();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    snapshot.state.set_draft_text(room, "brief").unwrap();
    let request = snapshot.state.submit_draft(room, 1000).unwrap()[0];
    snapshot
        .state
        .begin_submission(request, "launch", 0)
        .unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    (ui, room, agent, request)
}

fn failed(ui: &mut BusUi, request: RequestId) {
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .record_submission(
            request,
            SubmissionOutcome::Uncertain {
                message: DETAIL.into(),
            },
        )
        .unwrap();
    snapshot
        .state
        .fail_unconfirmed_submissions(1000 + DELIVERED_STALL_MS)
        .unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
}

#[test]
fn abandoned_submission_history_keeps_its_own_recovery_after_later_agent_errors() {
    let (mut ui, room, agent, request) = submitting();
    let mut history = super::super::history::History::default();
    let mut thumbnails = super::super::thumbnails::Thumbnails::default();
    history.lines(
        &ui.snapshot.state,
        ui.snapshot.state.room(room).unwrap(),
        200,
        1,
        2000,
        &mut thumbnails,
    );
    failed(&mut ui, request);
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .set_agent_error(agent, Some("a later unrelated error".into()))
        .unwrap();
    // The request outcome must survive snapshot persistence, not borrow the agent's latest error.
    snapshot.state = serde_json::from_slice(&serde_json::to_vec(&snapshot.state).unwrap()).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    let rendered = history
        .lines(
            &ui.snapshot.state,
            ui.snapshot.state.room(room).unwrap(),
            200,
            3,
            2000,
            &mut thumbnails,
        )
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for text in [
        "submission_unconfirmed",
        "draft was left",
        "before resending",
    ] {
        assert!(rendered.contains(text), "missing {text}: {rendered}");
    }
    assert!(
        !rendered.contains("a later unrelated error"),
        "outcomes belong to their request"
    );
}

#[test]
fn an_uncertain_submission_never_titles_the_composer() {
    let (mut ui, _, _, request) = submitting();
    failed(&mut ui, request);
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let mut title = String::new();
    for y in composer_rect(&ui).y..30 {
        for x in 0..100 {
            title.push_str(buffer[(x, y)].symbol());
        }
    }
    assert!(!title.contains("draft was left"), "{title}");
    assert!(!title.contains("submission_unconfirmed"), "{title}");
}

#[test]
fn expanded_details_show_uncertain_submission_recovery_without_a_startup_failure() {
    let (mut ui, _, agent, request) = submitting();
    failed(&mut ui, request);
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .set_agent_details_disclosed(agent, true)
        .unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.compute_view(160, 60);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 160, 60));
    ui.render(&mut buffer);
    let mut text = String::new();
    for y in ui.view.sidebar_body.y..ui.view.sidebar_body.bottom() {
        for x in 1..ui.view.sidebar.right() - 1 {
            text.push_str(buffer[(x, y)].symbol());
        }
    }
    for expected in ["draftwasleft", "beforeresending"] {
        assert!(text.replace(' ', "").contains(expected), "{text}");
    }
}
