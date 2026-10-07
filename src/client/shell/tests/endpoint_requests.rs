use super::*;

fn request_id(actions: &[ClientShellAction]) -> &str {
    let [ClientShellAction::Endpoint { request, .. }] = actions else {
        panic!("expected one endpoint request");
    };
    &request.id
}

fn focus_workspace(state: &mut ClientShellState) -> Vec<ClientShellAction> {
    let mut outcome = ClientShellInput::default();
    state.push_endpoint_method(
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "ws_1".into(),
        }),
        &mut outcome,
    );
    outcome.actions
}

struct TestTransport {
    fail: bool,
}

impl crate::client::endpoint::EndpointTransport for TestTransport {
    fn send(&mut self, _: &ClientMessage) -> std::io::Result<()> {
        if self.fail {
            Err(std::io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }
}

fn enqueue(
    commands: &mut crate::client::endpoint_commands::EndpointCommands,
    actions: Vec<ClientShellAction>,
) {
    for action in actions {
        let ClientShellAction::Endpoint { boot_id, request } = action else {
            panic!("expected endpoint request");
        };
        commands.enqueue(boot_id, request);
    }
}

#[test]
fn queued_requests_run_one_at_a_time_in_order() {
    use crate::client::endpoint::ServerConnection;
    use crate::client::endpoint_commands::EndpointCommands;

    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let first = focus_workspace(&mut state);
    let first_id = request_id(&first).to_owned();
    let second = focus_workspace(&mut state);
    let mut commands = EndpointCommands::default();
    enqueue(&mut commands, first);
    enqueue(&mut commands, second);
    let mut connection = ServerConnection::new(TestTransport { fail: false });

    assert!(commands.send_next(&mut connection).is_empty());
    assert!(commands.send_next(&mut connection).is_empty());
    let response = serde_json::to_vec(&crate::api::schema::SuccessResponse {
        id: first_id.clone(),
        result: crate::api::schema::ResponseResult::Ok {},
    })
    .unwrap();
    let completed = commands
        .receive_chunk("boot-1", &first_id, true, response)
        .expect("first request completes");
    assert_eq!(completed.request_id, first_id);
}

#[test]
fn a_failed_send_cancels_the_queued_requests() {
    use crate::client::endpoint::ServerConnection;
    use crate::client::endpoint_commands::EndpointCommands;

    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let first = focus_workspace(&mut state);
    let first_id = request_id(&first).to_owned();
    let second = focus_workspace(&mut state);
    let second_id = request_id(&second).to_owned();
    let mut commands = EndpointCommands::default();
    enqueue(&mut commands, first);
    enqueue(&mut commands, second);
    let mut connection = ServerConnection::new(TestTransport { fail: true });

    let cancelled = commands.send_next(&mut connection);
    assert_eq!(cancelled, vec![first_id.clone(), second_id.clone()]);
    assert!(connection.take_failure().is_some());
    for request_id in cancelled {
        assert!(state.cancel_endpoint_request(&request_id));
    }
    assert!(state.pending_requests.is_empty());
}

#[test]
fn failed_selection_copy_does_not_send_terminal_input() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.selection = Some(crate::selection::Selection::absolute_range(
        "pane_1".into(),
        (0, 0),
        (0, 2),
    ));
    for result in [
        Ok(crate::api::schema::ResponseResult::PaneSelection {
            pane_id: "pane_1".into(),
            text: String::new(),
        }),
        Err(ClientShellEndpointError {
            code: Some("endpoint_cancelled".into()),
            message: "cancelled".into(),
        }),
        Err(ClientShellEndpointError {
            code: Some("selection_unavailable".into()),
            message: "selection text is unavailable".into(),
        }),
    ] {
        let mut outcome = ClientShellInput::default();
        state.request_selection_copy(&mut outcome, false);
        let (_, actions) =
            state.handle_endpoint_result("boot-1", request_id(&outcome.actions), result);
        assert!(actions.is_empty());
        assert!(state.pending_requests.is_empty());
    }
}

#[test]
fn cancelled_link_activation_does_not_replay_mouse_input() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(100, 28).unwrap();
    let pane_id = state.hits.panes[0].pane_id.clone();
    let inner_rect = state.hits.panes[0].inner_rect;
    let mut outcome = ClientShellInput::default();
    state.push_endpoint_method_with_kind(
        crate::api::schema::Method::PaneLinkActivate(crate::api::schema::PaneLinkActivateParams {
            pane_id: pane_id.clone(),
            viewport_row: 0,
            col: 0,
            content_revision: None,
            offset_from_bottom: None,
        }),
        PendingEndpointKind::PaneLinkActivate {
            pane_id,
            inner_rect,
            fallback_events: vec![MouseEvent {
                kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: inner_rect.x,
                row: inner_rect.y,
                modifiers: KeyModifiers::NONE,
            }],
        },
        &mut outcome,
    );
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        request_id(&outcome.actions),
        Err(ClientShellEndpointError {
            code: Some("endpoint_cancelled".into()),
            message: "cancelled".into(),
        }),
    );
    assert!(actions.is_empty());
    assert!(state.url_click_consumes_until_up);
}
