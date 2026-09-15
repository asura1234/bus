use super::*;
use crate::bus::orchestrator::{
    ConfirmRoomBriefProposal, HumanOrchestratorMessage, ParticipantId, ProviderRequestSettlement,
    RoomMessage, RoomOperation, RoomRecipient, WorkflowDraftMutation,
};
use crate::bus::runtime::test_harness::TestWorkerHarness;
use crate::client::shell::bus::orchestrator_ui::{
    author_label, phase_reason, recipient_entries, redact_secret, settlement_label,
    settlement_reason, CoordinationAction, RecipientEntry, SettlementReason,
};
use crossterm::event::KeyCode;
use crossterm::event::KeyModifiers;
use std::sync::Arc;

/// Opens the harness room with the Orchestrator enabled in client settings.
fn open(harness: &TestWorkerHarness) -> BusUi {
    let mut ui = BusUi::new(Arc::new(harness.snapshot()));
    ui.settings.orchestrator.enabled = true;
    ui.open_room(harness.room());
    ui
}

/// Applies queued UI commands through the real Worker in order and publishes each result and
/// snapshot the way the coordinator loop does.
fn drain(ui: &mut BusUi, harness: &mut TestWorkerHarness) -> Vec<(BusCommand, Result<(), String>)> {
    let mut applied = Vec::new();
    for _ in 0..64 {
        ui.settle();
        let Some((id, command)) = ui
            .pending
            .iter()
            .find(|pending| pending.result.is_none())
            .map(|pending| (pending.id, pending.command.clone()))
        else {
            return applied;
        };
        let result = harness.command(command.clone());
        ui.receive_event(BusEvent::CommandFinished {
            command_id: id,
            result: result.clone(),
        });
        let mut snapshot = harness.snapshot();
        // The coordinator publishes the id of the UI command it has just applied.
        snapshot.last_command_id = id;
        ui.receive_snapshot(Arc::new(snapshot));
        applied.push((command, result));
    }
    panic!("UI commands did not settle");
}

/// Publishes Worker changes made outside the UI, such as a model-authored operation.
fn publish(ui: &mut BusUi, harness: &TestWorkerHarness) {
    let mut snapshot = harness.snapshot();
    snapshot.last_command_id = ui.snapshot.last_command_id;
    ui.receive_snapshot(Arc::new(snapshot));
}

fn compose(ui: &mut BusUi, room: RoomId, text: &str) {
    ui.locals.get_mut(&room).unwrap().text = editor::Editor::new(text.into());
    ui.text_changed(room);
}

fn rendered_action(ui: &BusUi, wanted: impl Fn(&render::Action) -> bool) -> render::Action {
    ui.view
        .hits
        .iter()
        .find(|hit| wanted(&hit.action))
        .map(|hit| hit.action.clone())
        .expect("action is rendered")
}

fn is_proposal_confirm(action: &render::Action) -> bool {
    matches!(
        action,
        render::Action::Coordination(CoordinationAction::ApproveProposal { .. })
    )
}

fn is_workflow_approval(action: &render::Action) -> bool {
    matches!(
        action,
        render::Action::Coordination(CoordinationAction::ApproveWorkflowPromotion(_))
    )
}

fn propose(harness: &mut TestWorkerHarness, expected_revision: u64, goal: &str) {
    harness
        .orchestrator_operation(RoomOperation::ProposeRoomBrief {
            expected_revision,
            goal: goal.into(),
            non_goals: "No repository changes by the Orchestrator".into(),
        })
        .unwrap();
}

fn persist_draft(
    harness: &mut TestWorkerHarness,
    draft_id: Option<&str>,
    expected_revision: Option<u64>,
    markdown: &str,
) -> serde_json::Value {
    harness
        .orchestrator_operation(RoomOperation::PersistWorkflowDraft(WorkflowDraftMutation {
            draft_id: draft_id.map(Into::into),
            expected_revision,
            workflow_id: draft_id.is_none().then(|| "release-sop".into()),
            markdown: markdown.into(),
        }))
        .unwrap()
}

fn submit_to_agent(harness: &mut TestWorkerHarness, text: &str) {
    let (room, agent) = (harness.room(), harness.agent());
    harness
        .command(BusCommand::SetRecipients(
            room,
            [agent].into_iter().collect(),
        ))
        .unwrap();
    harness
        .command(BusCommand::SetDraftText(room, text.into()))
        .unwrap();
    harness.command(BusCommand::Submit(room)).unwrap();
}

#[test]
fn room_orchestrator_ui_settings_redacts_api_key_and_keeps_color_blind_toggle() {
    let (mut ui, _, _) = fixture();
    let root = std::env::temp_dir().join(format!(
        "bus-ui-orch-root-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("settings.json");
    ui.settings_path = Some(path.clone());
    ui.action(render::Action::Settings);
    let screen = room_screen(&mut ui, 100, 30);
    assert!(screen.contains("Orchestrator"), "{screen}");
    assert!(screen.contains("API key"), "{screen}");
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    ui.input(
        &RawInputEvent::Paste("sk-live-secret-key".into()),
        false,
        &mut Default::default(),
    );
    let screen = room_screen(&mut ui, 100, 30);
    assert!(!screen.contains("sk-live-secret-key"), "{screen}");
    assert!(
        screen.contains(&redact_secret("sk-live-secret-key")),
        "{screen}"
    );
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    let screen = room_screen(&mut ui, 100, 30);
    assert!(!screen.contains("sk-live-secret-key"), "{screen}");
    assert!(screen.contains("stored digest"), "{screen}");
    let stored = std::fs::read(root.join("private/orchestrator-credentials.json")).unwrap();
    assert!(String::from_utf8_lossy(&stored).contains("sk-live-secret-key"));
    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.settings.color_blind_mode);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn room_orchestrator_ui_default_activation_lists_orchestrator_only_with_configured_key() {
    let mut harness = TestWorkerHarness::new();
    harness
        .command(BusCommand::CreateRoom("fresh".into()))
        .unwrap();
    let snapshot = harness.snapshot();
    let fresh = snapshot
        .state
        .rooms()
        .find(|room| room.name == "fresh")
        .unwrap()
        .id;
    let mut ui = BusUi::new(Arc::new(snapshot));
    ui.open_room(fresh);
    drain(&mut ui, &mut harness);
    let menu = |ui: &mut BusUi| {
        let hits = |ui: &mut BusUi, open: bool| {
            ui.recipient_menu = open;
            ui.compute_view(100, 40);
            ui.view
                .hits
                .iter()
                .map(|hit| (hit.rect, hit.action.clone()))
                .collect::<Vec<_>>()
        };
        let closed = hits(ui, false);
        let mut entries = hits(ui, true)
            .into_iter()
            .filter(|hit| {
                !closed.contains(hit)
                    && matches!(
                        hit.1,
                        render::Action::RecipientEntry(_) | render::Action::Recipient(_)
                    )
            })
            .collect::<Vec<_>>();
        ui.recipient_menu = false;
        entries.sort_by_key(|(rect, _)| rect.y);
        entries
            .into_iter()
            .map(|(_, action)| action)
            .collect::<Vec<_>>()
    };

    assert!(!ui.settings.orchestrator.enabled);
    assert_eq!(menu(&mut ui), vec![render::Action::Recipient(None)]);

    let root = std::env::temp_dir().join(format!(
        "bus-ui-orch-key-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    ui.settings_path = Some(root.join("settings.json"));
    ui.settings_key = editor::Editor::new("sk-test-orchestrator-key".into());
    ui.save_orchestrator_key();
    assert!(ui.settings.orchestrator.enabled, "{:?}", ui.error);
    assert_eq!(
        menu(&mut ui),
        vec![
            render::Action::RecipientEntry(RecipientEntry::Orchestrator),
            render::Action::Recipient(None),
        ]
    );
    assert!(ui.pending.is_empty());
    let state = harness.snapshot().state;
    assert_eq!(state.requests().count(), 0);
    assert_eq!(state.room_messages(fresh).count(), 0);
    assert!(state.room_brief_confirmation(fresh).is_none());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn room_orchestrator_ui_recipient_order_keeps_all_to_coding_agents_and_authors_distinct() {
    let mut harness = TestWorkerHarness::new();
    let (room, agent) = (harness.room(), harness.agent());
    assert_eq!(
        recipient_entries(true, [agent]),
        [
            RecipientEntry::Orchestrator,
            RecipientEntry::Separator,
            RecipientEntry::AllAgents,
            RecipientEntry::Agent(agent),
        ]
    );
    assert_eq!(
        recipient_entries(false, [agent]),
        [RecipientEntry::AllAgents, RecipientEntry::Agent(agent)]
    );
    let mut ui = open(&harness);
    drain(&mut ui, &mut harness);
    ui.action(render::Action::Recipient(None));
    drain(&mut ui, &mut harness);
    let selected = [agent].into_iter().collect::<AgentRecipients>();
    assert_eq!(
        ui.snapshot.state.room(room).unwrap().draft.recipient_ids,
        selected
    );
    assert!(!ui.locals[&room].to_orchestrator);
    ui.action(render::Action::RecipientEntry(RecipientEntry::Orchestrator));
    assert!(ui.locals[&room].to_orchestrator);
    assert_eq!(ui.locals[&room].recipients, selected);
    assert!(ui.pending.is_empty());

    assert_eq!(
        author_label(&ParticipantId::Human, |_| unreachable!()),
        "You"
    );
    assert_eq!(
        author_label(&ParticipantId::Orchestrator, |_| unreachable!()),
        "Orchestrator"
    );
    assert_eq!(
        author_label(&ParticipantId::Agent(agent), |_| "agent".into()),
        "agent"
    );
}

#[test]
fn room_orchestrator_ui_history_interleaves_room_messages_and_prompts_by_shared_identity() {
    let mut harness = TestWorkerHarness::new();
    let (room, agent) = (harness.room(), harness.agent());
    harness
        .command(BusCommand::MessageOrchestrator(HumanOrchestratorMessage {
            room_id: room,
            body: "alpha human asks orchestrator".into(),
        }))
        .unwrap();
    submit_to_agent(&mut harness, "bravo human assigns agent");
    harness
        .orchestrator_operation(RoomOperation::SendMessage(RoomMessage {
            author: ParticipantId::Orchestrator,
            to: RoomRecipient::Human,
            text: "charlie orchestrator asks human".into(),
            work: None,
        }))
        .unwrap();
    harness
        .orchestrator_operation(RoomOperation::SendMessage(RoomMessage {
            author: ParticipantId::Orchestrator,
            to: RoomRecipient::Agent(agent),
            text: "delta orchestrator assigns agent".into(),
            work: None,
        }))
        .unwrap();
    let state = harness.snapshot().state;
    let messages = state
        .room_messages(room)
        .map(|record| record.message_id.0)
        .collect::<Vec<_>>();
    let prompts = state
        .requests()
        .filter(|request| request.room_id == room)
        .map(|request| request.prompt.id.0)
        .collect::<Vec<_>>();
    assert_eq!((messages.len(), prompts.len()), (2, 2));
    assert!(messages[0] < prompts[0] && prompts[0] < messages[1] && messages[1] < prompts[1]);

    let mut ui = open(&harness);
    drain(&mut ui, &mut harness);
    let screen = room_screen(&mut ui, 120, 50);
    let positions = [
        "alpha human asks orchestrator",
        "bravo human assigns agent",
        "charlie orchestrator asks human",
        "delta orchestrator assigns agent",
    ]
    .map(|text| {
        screen
            .find(text)
            .unwrap_or_else(|| panic!("{text} is missing: {screen}"))
    });
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{screen}"
    );
    for header in [
        "You → Orchestrator",
        "You → agent",
        "Orchestrator → You",
        "Orchestrator → agent",
    ] {
        assert!(screen.contains(header), "{header}: {screen}");
    }
}

#[test]
fn room_orchestrator_ui_orchestrator_only_send_dispatches_exact_body_without_agent_submit() {
    let mut harness = TestWorkerHarness::new();
    let (room, agent) = (harness.room(), harness.agent());
    let mut ui = open(&harness);
    drain(&mut ui, &mut harness);
    ui.action(render::Action::RecipientEntry(RecipientEntry::Orchestrator));
    let body = "  Where are we?\nList blockers only.  ";
    compose(&mut ui, room, body);
    ui.request_send(room);
    let applied = drain(&mut ui, &mut harness);
    let messages = applied
        .iter()
        .filter_map(|(command, result)| match command {
            BusCommand::MessageOrchestrator(message) => Some((message.clone(), result.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        messages,
        vec![(
            HumanOrchestratorMessage {
                room_id: room,
                body: body.into(),
            },
            Ok(())
        )]
    );
    assert!(!applied
        .iter()
        .any(|(command, _)| matches!(command, BusCommand::Submit(_))));
    let state = &ui.snapshot.state;
    assert_eq!(
        state
            .room_messages(room)
            .map(|record| (record.message.to.clone(), record.message.text.as_str()))
            .collect::<Vec<_>>(),
        vec![(RoomRecipient::Orchestrator, body)]
    );
    assert_eq!(state.requests().count(), 0);
    assert!(ui.locals[&room].text.text.is_empty());
    assert!(state.room(room).unwrap().draft.text.is_empty());

    ui.action(render::Action::Recipient(Some(agent)));
    let both = "Both of you: confirm the plan.";
    compose(&mut ui, room, both);
    ui.request_send(room);
    let applied = drain(&mut ui, &mut harness);
    let sends = applied
        .iter()
        .filter(|(command, _)| {
            matches!(
                command,
                BusCommand::MessageOrchestrator(_) | BusCommand::Submit(_)
            )
        })
        .map(|(command, result)| (matches!(command, BusCommand::Submit(_)), result.clone()))
        .collect::<Vec<_>>();
    assert_eq!(sends, vec![(false, Ok(())), (true, Ok(()))]);
    let state = &ui.snapshot.state;
    assert_eq!(state.room_messages(room).last().unwrap().message.text, both);
    let requests = state.requests().collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt.text, both);
    assert_eq!(requests[0].prompt.author, ParticipantId::Human);
    assert!(ui.locals[&room].text.text.is_empty());
}

#[test]
fn room_orchestrator_ui_proposal_confirm_dispatches_rendered_revision_and_digest() {
    let mut harness = TestWorkerHarness::new();
    let room = harness.room();
    propose(&mut harness, 0, "Ship the reviewed room workflow");
    let (revision, digest) = harness.snapshot().state.room_brief_proposal(room).unwrap();
    let mut ui = open(&harness);
    drain(&mut ui, &mut harness);
    let screen = room_screen(&mut ui, 120, 40);
    assert!(
        screen.contains("Goal: Ship the reviewed room workflow"),
        "{screen}"
    );
    assert!(
        screen.contains("Non-goals: No repository changes by the Orchestrator"),
        "{screen}"
    );
    let confirm = rendered_action(&ui, is_proposal_confirm);
    assert_eq!(
        confirm,
        render::Action::Coordination(CoordinationAction::ApproveProposal {
            revision,
            digest: digest.clone(),
        })
    );
    ui.action(confirm);
    let applied = drain(&mut ui, &mut harness);
    let confirmations = applied
        .iter()
        .filter_map(|(command, result)| match command {
            BusCommand::ConfirmRoomBriefProposal(command) => {
                Some((command.clone(), result.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        confirmations,
        vec![(
            ConfirmRoomBriefProposal {
                room_id: room,
                expected_developer: ParticipantId::Human,
                expected_proposal_revision: revision,
                expected_proposal_digest: digest.clone(),
            },
            Ok(())
        )]
    );
    let receipt = ui.snapshot.state.room_brief_confirmation(room).unwrap();
    assert_eq!(
        (receipt.approved_revision, receipt.proposal_digest.as_str()),
        (revision, digest.as_str())
    );
    assert!(ui.snapshot.state.room(room).unwrap().brief.locked);
    let screen = room_screen(&mut ui, 120, 40);
    assert!(
        !ui.view
            .hits
            .iter()
            .any(|hit| is_proposal_confirm(&hit.action)),
        "{screen}"
    );
}

#[test]
fn room_orchestrator_ui_newer_proposal_after_display_surfaces_worker_stale_rejection() {
    let mut harness = TestWorkerHarness::new();
    let room = harness.room();
    propose(&mut harness, 0, "Displayed goal");
    let mut ui = open(&harness);
    drain(&mut ui, &mut harness);
    room_screen(&mut ui, 120, 40);
    let displayed = rendered_action(&ui, is_proposal_confirm);
    let render::Action::Coordination(CoordinationAction::ApproveProposal { revision, digest }) =
        displayed.clone()
    else {
        unreachable!()
    };

    propose(&mut harness, revision, "Adapted goal");
    ui.action(displayed.clone());
    let applied = drain(&mut ui, &mut harness);
    let confirmations = applied
        .iter()
        .filter_map(|(command, result)| match command {
            BusCommand::ConfirmRoomBriefProposal(command) => {
                Some((command.clone(), result.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        confirmations.len(),
        1,
        "a stale proposal is never resubmitted"
    );
    assert_eq!(
        (
            confirmations[0].0.expected_proposal_revision,
            confirmations[0].0.expected_proposal_digest.as_str()
        ),
        (revision, digest.as_str())
    );
    let rejection = confirmations[0].1.clone().unwrap_err();
    assert_eq!(ui.error.as_deref(), Some(rejection.as_str()));
    assert!(ui.pending.is_empty());
    let state = &ui.snapshot.state;
    assert!(state.room_brief_confirmation(room).is_none());
    assert!(!state.room(room).unwrap().brief.locked);
    assert_eq!(state.room_brief_proposal(room).unwrap().0, revision + 1);

    let screen = room_screen(&mut ui, 120, 40);
    assert!(screen.contains("Goal: Adapted goal"), "{screen}");
    assert_ne!(rendered_action(&ui, is_proposal_confirm), displayed);
}

#[test]
fn room_orchestrator_ui_workflow_review_renders_worker_diff_and_dispatches_approval_command() {
    let mut harness = TestWorkerHarness::new();
    let room = harness.room();
    persist_draft(
        &mut harness,
        None,
        None,
        "# Release SOP\n\n## Verify\n\nRead evidence.\n",
    );
    let review = harness.snapshot().state.workflow_promotion_reviews(room)[0].clone();
    let mut ui = open(&harness);
    drain(&mut ui, &mut harness);
    let screen = room_screen(&mut ui, 160, 50);
    for expected in [
        format!(
            "Review {} · draft {} rev {}",
            review.workflow_id, review.draft_id, review.draft_revision
        ),
        format!("content {}", review.content_digest),
        "base absent".into(),
        format!("diff {}", review.reviewed_diff_digest),
        "+# Release SOP".into(),
        "+Read evidence.".into(),
    ] {
        assert!(screen.contains(&expected), "{expected}: {screen}");
    }
    let approve = rendered_action(&ui, is_workflow_approval);
    assert_eq!(
        approve,
        render::Action::Coordination(CoordinationAction::ApproveWorkflowPromotion(Box::new(
            review.clone()
        )))
    );
    ui.action(approve);
    let applied = drain(&mut ui, &mut harness);
    let approvals = applied
        .iter()
        .filter_map(|(command, result)| match command {
            BusCommand::CreateDeveloperWorkflowApproval(command) => {
                Some((command.clone(), result.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(approvals, vec![(review.approval_command(), Ok(()))]);
    assert_eq!(
        ui.snapshot.state.developer_workflow_approvals(room).len(),
        1
    );
}

fn screen_rows(screen: &str, cols: u16) -> Vec<String> {
    screen
        .chars()
        .collect::<Vec<_>>()
        .chunks(usize::from(cols))
        .map(|row| row.iter().collect())
        .collect()
}

#[test]
fn room_orchestrator_ui_workflow_approval_is_hidden_until_the_full_review_is_visible() {
    let hint = "diff is not fully shown; enlarge the terminal to approve";
    let long_sop = format!(
        "# Long SOP\n\n## Steps\n\n{}",
        (0..60)
            .map(|index| format!("Step {index:02} reads evidence.\n"))
            .collect::<String>()
    );
    for (markdown, cols, rows) in [
        (long_sop.as_str(), 160, 50),
        ("# Release SOP\n\n## Verify\n\nRead evidence.\n", 160, 20),
    ] {
        let mut harness = TestWorkerHarness::new();
        let room = harness.room();
        persist_draft(&mut harness, None, None, markdown);
        let review = harness.snapshot().state.workflow_promotion_reviews(room)[0].clone();
        let mut ui = open(&harness);
        drain(&mut ui, &mut harness);
        let screen = room_screen(&mut ui, cols, rows);
        assert!(
            !ui.view
                .hits
                .iter()
                .any(|hit| is_workflow_approval(&hit.action)),
            "{cols}x{rows}: {screen}"
        );
        assert!(
            !screen.contains("Approve workflow"),
            "{cols}x{rows}: {screen}"
        );
        let lines = screen_rows(&screen, cols);
        let hint_y = lines
            .iter()
            .position(|line| line.contains(hint))
            .unwrap_or_else(|| panic!("{cols}x{rows}: {screen}"));
        assert!(lines[hint_y].contains(&review.workflow_id), "{screen}");
        let hint_x = lines[hint_y].find(hint).unwrap();
        let hint_x = lines[hint_y][..hint_x].chars().count() as u16;
        assert!(
            !ui.view.hits.iter().any(|hit| {
                hit.rect.y == hint_y as u16 && hit.rect.x <= hint_x && hint_x < hit.rect.right()
            }),
            "the truncation notice is not clickable: {screen}"
        );
        assert!(ui.pending.is_empty());
    }
}

#[test]
fn room_orchestrator_ui_workflow_approval_follows_the_fully_visible_diff() {
    let mut harness = TestWorkerHarness::new();
    let room = harness.room();
    persist_draft(
        &mut harness,
        None,
        None,
        "# Release SOP\n\n## Verify\n\nRead evidence.\n",
    );
    let review = harness.snapshot().state.workflow_promotion_reviews(room)[0].clone();
    let mut ui = open(&harness);
    drain(&mut ui, &mut harness);
    let cols = 160;
    let screen = room_screen(&mut ui, cols, 50);
    let lines = screen_rows(&screen, cols);
    let digest = format!("diff {}", review.reviewed_diff_digest);
    let digest_y = lines
        .iter()
        .position(|line| line.contains(&digest))
        .unwrap_or_else(|| panic!("{screen}"));
    let column = lines[digest_y][..lines[digest_y].find(&digest).unwrap()]
        .chars()
        .count();
    let diff = review.rendered_diff.lines().collect::<Vec<_>>();
    for (offset, diff_line) in diff.iter().enumerate() {
        let row = lines[digest_y + 1 + offset]
            .chars()
            .skip(column)
            .collect::<String>();
        assert!(row.starts_with(diff_line), "{diff_line}: {screen}");
    }
    let approvals = ui
        .view
        .hits
        .iter()
        .filter(|hit| is_workflow_approval(&hit.action))
        .collect::<Vec<_>>();
    assert_eq!(approvals.len(), 1, "{screen}");
    let approve_y = usize::from(approvals[0].rect.y);
    assert_eq!(approve_y, digest_y + 1 + diff.len(), "{screen}");
    assert!(
        lines[approve_y].contains(&format!("Approve workflow {}", review.workflow_id)),
        "{screen}"
    );
}

#[test]
fn room_orchestrator_ui_stale_workflow_review_surfaces_worker_rejection_without_resubmission() {
    let mut harness = TestWorkerHarness::new();
    let room = harness.room();
    let created = persist_draft(
        &mut harness,
        None,
        None,
        "# Release SOP\n\n## Verify\n\nRead evidence.\n",
    );
    let draft_id = created["receipt"]["draft_id"].as_str().unwrap().to_owned();
    let mut ui = open(&harness);
    drain(&mut ui, &mut harness);
    room_screen(&mut ui, 160, 50);
    let displayed = rendered_action(&ui, is_workflow_approval);
    let render::Action::Coordination(CoordinationAction::ApproveWorkflowPromotion(review)) =
        displayed.clone()
    else {
        unreachable!()
    };

    persist_draft(
        &mut harness,
        Some(&draft_id),
        Some(1),
        "# Release SOP\n\n## Verify\n\nRead revised evidence.\n",
    );
    ui.action(displayed);
    let applied = drain(&mut ui, &mut harness);
    let approvals = applied
        .iter()
        .filter_map(|(command, result)| match command {
            BusCommand::CreateDeveloperWorkflowApproval(command) => {
                Some((command.clone(), result.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(approvals.len(), 1, "a stale review is never resubmitted");
    assert_eq!(approvals[0].0, review.approval_command());
    let rejection = approvals[0].1.clone().unwrap_err();
    assert!(rejection.contains("StaleApproval"), "{rejection}");
    assert_eq!(ui.error.as_deref(), Some(rejection.as_str()));
    assert!(ui.pending.is_empty());
    assert!(ui
        .snapshot
        .state
        .developer_workflow_approvals(room)
        .is_empty());
    let screen = room_screen(&mut ui, 160, 50);
    assert!(
        screen.contains(&format!("draft {draft_id} rev 2")),
        "{screen}"
    );
}

#[test]
fn room_orchestrator_ui_restart_renders_proposal_and_workflow_receipts_from_worker_state() {
    let mut harness = TestWorkerHarness::new();
    let room = harness.room();
    propose(&mut harness, 0, "Restartable goal");
    persist_draft(
        &mut harness,
        None,
        None,
        "# Release SOP\n\n## Verify\n\nRead evidence.\n",
    );
    let mut ui = open(&harness);
    drain(&mut ui, &mut harness);
    room_screen(&mut ui, 160, 50);
    ui.action(rendered_action(&ui, is_proposal_confirm));
    drain(&mut ui, &mut harness);
    room_screen(&mut ui, 160, 50);
    ui.action(rendered_action(&ui, is_workflow_approval));
    drain(&mut ui, &mut harness);
    let receipt = ui
        .snapshot
        .state
        .room_brief_confirmation(room)
        .unwrap()
        .clone();
    let approval = ui.snapshot.state.developer_workflow_approvals(room)[0].clone();

    harness.restart();
    let mut ui = open(&harness);
    drain(&mut ui, &mut harness);
    let screen = room_screen(&mut ui, 160, 50);
    assert!(
        screen.contains(&format!(
            "Confirmed rev {} {}",
            receipt.approved_revision, receipt.proposal_digest
        )),
        "{screen}"
    );
    assert!(
        screen.contains(&format!(
            "Approved {} rev {} {}",
            approval.workflow_id, approval.draft_revision, approval.approval_id
        )),
        "{screen}"
    );
    assert!(!ui
        .view
        .hits
        .iter()
        .any(|hit| is_proposal_confirm(&hit.action)));
}

#[test]
fn room_orchestrator_ui_abandoned_settlement_is_labeled_abandoned_not_wedged() {
    let labels = [
        ProviderRequestSettlement::Queued,
        ProviderRequestSettlement::Submitting,
        ProviderRequestSettlement::Active,
        ProviderRequestSettlement::Completed,
        ProviderRequestSettlement::Abandoned,
    ]
    .map(|phase| settlement_label(phase_reason(phase, false, true)));
    assert_eq!(
        labels,
        ["queued", "launching", "active", "settled", "abandoned"]
    );
    assert_eq!(
        phase_reason(ProviderRequestSettlement::Abandoned, false, true),
        SettlementReason::Abandoned
    );
    assert_eq!(
        settlement_label(phase_reason(
            ProviderRequestSettlement::Active,
            false,
            false
        )),
        "hook"
    );
    assert_eq!(
        settlement_label(phase_reason(
            ProviderRequestSettlement::Completed,
            true,
            true
        )),
        "uncertain"
    );

    let mut harness = TestWorkerHarness::new();
    submit_to_agent(&mut harness, "label source");
    let mut ui = open(&harness);
    drain(&mut ui, &mut harness);
    let request = ui.snapshot.state.requests().next().unwrap().id;
    let settlement = ui.snapshot.state.work_settlement(request).unwrap();
    assert_eq!(settlement_label(settlement_reason(&settlement)), "queued");
    let screen = room_screen(&mut ui, 120, 40);
    assert!(screen.contains("  queued"), "{screen}");
    assert!(!screen.contains("wedged"), "{screen}");
}

#[test]
fn room_orchestrator_ui_narrow_and_short_layouts_keep_composer_and_conversation() {
    let mut harness = TestWorkerHarness::new();
    let room = harness.room();
    propose(&mut harness, 0, "Layout goal");
    persist_draft(
        &mut harness,
        None,
        None,
        "# Release SOP\n\n## Verify\n\nRead evidence.\n",
    );
    harness
        .command(BusCommand::MessageOrchestrator(HumanOrchestratorMessage {
            room_id: room,
            body: "layout ping".into(),
        }))
        .unwrap();
    let mut ui = open(&harness);
    drain(&mut ui, &mut harness);
    for (cols, rows) in [(48, 18), (100, 14), (48, 14)] {
        let screen = room_screen(&mut ui, cols, rows);
        assert!(
            ui.view
                .hits
                .iter()
                .any(|hit| hit.action == render::Action::Composer),
            "{cols}x{rows}: {screen}"
        );
        assert!(ui.view.history.height > 0, "{cols}x{rows}: {screen}");
        assert!(
            ui.view.history.bottom() <= ui.view.composer.y,
            "{cols}x{rows}: {screen}"
        );
        assert!(screen.contains("layout ping"), "{cols}x{rows}: {screen}");
    }
}

#[test]
fn room_orchestrator_ui_color_blind_and_history_scroll_survive_orchestrator_chrome() {
    let mut harness = TestWorkerHarness::new();
    for index in 0..8 {
        submit_to_agent(&mut harness, &format!("prompt-{index:02}"));
    }
    let mut ui = open(&harness);
    drain(&mut ui, &mut harness);
    ui.action(render::Action::Settings);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.settings.color_blind_mode);
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    drain(&mut ui, &mut harness);
    ui.history_follow_tail = false;
    ui.main_scroll = 0;
    let screen = room_screen(&mut ui, 100, 40);
    assert!(screen.contains("prompt-00"), "{screen}");
    assert!(screen.contains("You"), "{screen}");
}

#[test]
fn room_orchestrator_ui_has_no_client_owned_store_poller_or_fake_panels() {
    let mut harness = TestWorkerHarness::new();
    let room = harness.room();
    propose(&mut harness, 0, "Visible goal");
    let mut ui = open(&harness);
    drain(&mut ui, &mut harness);
    room_screen(&mut ui, 120, 40);
    for _ in 0..3 {
        ui.tick();
        room_screen(&mut ui, 120, 40);
    }
    assert!(
        ui.pending.is_empty(),
        "rendering and ticks never issue commands"
    );

    propose(&mut harness, 1, "Worker adapted goal");
    publish(&mut ui, &harness);
    let screen = room_screen(&mut ui, 120, 40);
    assert!(screen.contains("Goal: Worker adapted goal"), "{screen}");
    assert!(!screen.contains("Visible goal"), "{screen}");
    let (revision, digest) = ui.snapshot.state.room_brief_proposal(room).unwrap();
    assert_eq!(
        rendered_action(&ui, is_proposal_confirm),
        render::Action::Coordination(CoordinationAction::ApproveProposal { revision, digest })
    );

    let sources = [
        include_str!("orchestrator_ui.rs"),
        include_str!("render.rs"),
        include_str!("state.rs"),
        include_str!("history.rs"),
        include_str!("input.rs"),
    ];
    for source in sources {
        for removed in [
            "HumanTaskId",
            "ResolveHumanTask",
            "RejectProposal",
            "CoordinationPanel",
            "intent_digest",
            "operations()",
            "todo_lines",
            "Wedged",
        ] {
            assert!(
                !source.contains(removed),
                "{removed} remains in the room UI"
            );
        }
    }
    for source in [
        include_str!("orchestrator_ui.rs"),
        include_str!("render.rs"),
        include_str!("history.rs"),
    ] {
        assert!(!source.contains("serde_json"));
    }
}
