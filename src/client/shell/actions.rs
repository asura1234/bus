use super::*;

impl ClientShellState {
    pub(super) fn request_selection_copy(&mut self, outcome: &mut ClientShellInput, live: bool) {
        let Some(selection) = self.selection.as_ref() else {
            return;
        };
        let pane_id = selection.pane_id.clone();
        let content_revision = self
            .pane_surface
            .as_ref()
            .and_then(|surface| surface.panes.iter().find(|pane| pane.pane_id == pane_id))
            .map(|pane| pane.content_revision)
            // Read a manual mouse selection atomically from the live terminal. Output
            // between the displayed frame and this request must not reject the copy.
            .filter(|_| !live);
        let (anchor, cursor) = selection.ordered_cells();
        self.push_endpoint_method_with_kind(
            crate::api::schema::Method::PaneSelectionRead(
                crate::api::schema::PaneSelectionReadParams {
                    pane_id,
                    anchor: crate::api::schema::PaneTextPoint {
                        row: anchor.0,
                        col: anchor.1,
                    },
                    cursor: crate::api::schema::PaneTextPoint {
                        row: cursor.0,
                        col: cursor.1,
                    },
                    content_revision,
                },
            ),
            PendingEndpointKind::SelectionCopy,
            outcome,
        );
    }

    pub(super) fn request_word_selection(
        &mut self,
        hit: &PaneHit,
        viewport_row: u16,
        col: u16,
        outcome: &mut ClientShellInput,
    ) {
        let absolute_row = crate::selection::absolute_row_for_viewport(viewport_row, hit.scroll);
        let content_revision = self
            .pane_surface
            .as_ref()
            .and_then(|surface| {
                surface
                    .panes
                    .iter()
                    .find(|pane| pane.pane_id == hit.pane_id)
            })
            .map(|pane| pane.content_revision);
        self.word_selection_generation = self.word_selection_generation.saturating_add(1);
        let generation = self.word_selection_generation;
        self.pending_word_selection = Some(generation);
        if !self.push_endpoint_method_with_kind(
            crate::api::schema::Method::PaneSelectionRead(
                crate::api::schema::PaneSelectionReadParams {
                    pane_id: hit.pane_id.clone(),
                    anchor: crate::api::schema::PaneTextPoint {
                        row: absolute_row,
                        col: 0,
                    },
                    cursor: crate::api::schema::PaneTextPoint {
                        row: absolute_row,
                        col: hit.inner_rect.width.saturating_sub(1),
                    },
                    content_revision,
                },
            ),
            PendingEndpointKind::WordSelection {
                pane_id: hit.pane_id.clone(),
                absolute_row,
                col,
                generation,
            },
            outcome,
        ) {
            self.pending_word_selection = None;
        }
    }

    pub(super) fn push_endpoint_method(
        &mut self,
        method: crate::api::schema::Method,
        outcome: &mut ClientShellInput,
    ) {
        self.push_endpoint_method_with_kind(method, PendingEndpointKind::Generic, outcome);
    }

    fn push_endpoint_notice(
        &mut self,
        kind: ClientEndpointNoticeKind,
        code: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> bool {
        let key = ClientEndpointNoticeKey {
            boot_id: self
                .snapshot
                .as_deref()
                .map(|snapshot| snapshot.boot_id.clone())
                .unwrap_or_else(|| "disconnected".to_owned()),
            kind,
            code: code.into(),
        };
        let body = body.into();
        if kind == ClientEndpointNoticeKind::Rejected {
            if self
                .visible_endpoint_notice
                .as_ref()
                .is_some_and(|notice| notice.key == key && notice.body == body)
            {
                return false;
            }
        } else if !self.endpoint_notice_seen.insert(key.clone()) {
            return false;
        }
        let duration_seconds = if kind == ClientEndpointNoticeKind::Rejected {
            3
        } else {
            8
        };
        self.visible_endpoint_notice = Some(ClientVisibleEndpointNotice {
            key,
            title: title.into(),
            body,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(duration_seconds),
        });
        true
    }

    pub(super) fn push_endpoint_method_with_kind(
        &mut self,
        method: crate::api::schema::Method,
        kind: PendingEndpointKind,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !self.endpoint_is_online(&self.active_endpoint_id) {
            let label = self.active_endpoint_label().to_owned();
            outcome.repaint |= self.receive_endpoint_unavailable(format!("{label} is not ready"));
            return false;
        }
        let method_name = crate::api::api_method_name(&method).to_owned();
        if !self.supports_endpoint_method(&method) {
            outcome.repaint |= self.push_endpoint_notice(
                ClientEndpointNoticeKind::Unsupported,
                method_name.clone(),
                "Action unavailable",
                format!(
                    "This server does not support {method_name} yet. Update and restart it to enable this action."
                ),
            );
            return false;
        }
        let Some(snapshot) = self.snapshot.as_deref() else {
            return false;
        };
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        let request_id = format!("client-shell:{request_id}");
        self.pending_requests.insert(
            request_id.clone(),
            PendingEndpointRequest {
                boot_id: snapshot.boot_id.clone(),
                method_name,
                kind,
            },
        );
        outcome.actions.push(ClientShellAction::Endpoint {
            endpoint_id: self.active_endpoint_id.clone(),
            boot_id: snapshot.boot_id.clone(),
            request: Box::new(crate::api::schema::Request {
                id: request_id,
                method,
            }),
        });
        true
    }

    pub(crate) fn receive_endpoint_error(&mut self, message: String) -> bool {
        self.push_endpoint_notice(
            ClientEndpointNoticeKind::Rejected,
            "paste_rejected",
            "Paste rejected",
            message,
        )
    }

    pub(crate) fn receive_endpoint_unavailable(&mut self, message: String) -> bool {
        self.push_endpoint_notice(
            ClientEndpointNoticeKind::Unavailable,
            message.clone(),
            "Endpoint unavailable",
            message,
        )
    }

    pub(crate) fn cancel_endpoint_request(&mut self, request_id: &str) -> bool {
        let Some(pending) = self.pending_requests.get(request_id) else {
            return false;
        };
        let boot_id = pending.boot_id.clone();
        let (repaint, actions) = self.handle_endpoint_result(
            &boot_id,
            request_id,
            Err(ClientShellEndpointError {
                code: Some("endpoint_cancelled".into()),
                message: "This server action was interrupted. Check its state before retrying."
                    .into(),
            }),
        );
        debug_assert!(
            actions.is_empty(),
            "cancellation must not start another action"
        );
        repaint
    }

    pub(crate) fn handle_endpoint_result(
        &mut self,
        boot_id: &str,
        request_id: &str,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let Some(pending) = self.pending_requests.remove(request_id) else {
            return (false, Vec::new());
        };
        if pending.boot_id != boot_id
            || self
                .snapshot
                .as_deref()
                .is_none_or(|snapshot| snapshot.boot_id != boot_id)
        {
            return (false, Vec::new());
        }
        if result.is_ok() {
            let timeout_key = ClientEndpointNoticeKey {
                boot_id: boot_id.to_owned(),
                kind: ClientEndpointNoticeKind::Timeout,
                code: pending.method_name.clone(),
            };
            self.endpoint_notice_seen.remove(&timeout_key);
        }
        if let Err(error) = &result {
            let code = error.code.as_deref().unwrap_or("invalid_response");
            if !matches!(
                code,
                "confirmation_required" | "stale_content" | "stale_target"
            ) {
                let (kind, notice_code, title, body) = match code {
                    "endpoint_timeout" => (
                        ClientEndpointNoticeKind::Timeout,
                        pending.method_name.clone(),
                        "Server timed out",
                        format!("This server did not respond to {}.", pending.method_name),
                    ),
                    "endpoint_cancelled" => (
                        ClientEndpointNoticeKind::Unavailable,
                        "cancelled".to_owned(),
                        "Action interrupted",
                        error.message.clone(),
                    ),
                    "server_unavailable" => (
                        ClientEndpointNoticeKind::Unavailable,
                        "server".to_owned(),
                        "Server unavailable",
                        error.message.clone(),
                    ),
                    _ => (
                        ClientEndpointNoticeKind::Rejected,
                        format!("{}:{code}", pending.method_name),
                        "Action rejected",
                        error.message.clone(),
                    ),
                };
                self.push_endpoint_notice(kind, notice_code, title, body);
            }
        }
        match pending.kind {
            PendingEndpointKind::Generic => {}
            PendingEndpointKind::PaneScroll { pane_id, serial } => {
                let mut outcome = ClientShellInput::default();
                let repaint = self.complete_pane_scroll(pane_id, serial, result, &mut outcome);
                return (repaint, outcome.actions);
            }
            PendingEndpointKind::SelectionCopy => {
                return match result {
                    Ok(crate::api::schema::ResponseResult::PaneSelection { text, .. })
                        if !text.is_empty() =>
                    {
                        let repaint = self.show_copy_feedback(std::time::Instant::now());
                        (
                            repaint,
                            vec![ClientShellAction::ClipboardWrite(text.into_bytes())],
                        )
                    }
                    Ok(crate::api::schema::ResponseResult::PaneSelection { .. }) => {
                        (false, Vec::new())
                    }
                    Ok(_) => {
                        self.endpoint_error =
                            Some("endpoint returned an unexpected selection result".to_owned());
                        (true, Vec::new())
                    }
                    Err(_) => (true, Vec::new()),
                };
            }
            PendingEndpointKind::WordSelection {
                pane_id,
                absolute_row,
                col,
                generation,
            } => {
                if self.pending_word_selection != Some(generation)
                    || self.snapshot.as_deref().is_none_or(|snapshot| {
                        !snapshot.panes.iter().any(|pane| pane.pane_id == pane_id)
                    })
                {
                    return (false, Vec::new());
                }
                self.pending_word_selection = None;
                let row_text = match result {
                    Ok(crate::api::schema::ResponseResult::PaneSelection {
                        pane_id: returned_pane_id,
                        text,
                    }) if returned_pane_id == pane_id => text,
                    Ok(crate::api::schema::ResponseResult::PaneSelection { .. }) => {
                        return (false, Vec::new())
                    }
                    Ok(_) => {
                        self.endpoint_error = Some(
                            "endpoint returned an unexpected word-selection result".to_owned(),
                        );
                        return (true, Vec::new());
                    }
                    Err(_) => return (true, Vec::new()),
                };
                let Some((start_col, end_col)) =
                    crate::app::actions::word_bounds_at_column(&row_text, col)
                else {
                    self.selection = None;
                    return (true, Vec::new());
                };
                let mut selection = crate::selection::Selection::absolute_range(
                    pane_id,
                    (absolute_row, start_col),
                    (absolute_row, end_col),
                );
                if !selection.finish() {
                    return (false, Vec::new());
                }
                self.selection = Some(selection);
                self.selection_autoscroll = None;
                self.selection_autoscroll_deadline = None;
                if !self.config.copy_on_select {
                    return (true, Vec::new());
                }
                self.selection_highlight_clear_deadline =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                let mut outcome = ClientShellInput::default();
                self.request_selection_copy(&mut outcome, false);
                return (true, outcome.actions);
            }
            PendingEndpointKind::PaneLinkActivate {
                pane_id,
                inner_rect,
                fallback_events,
            } => {
                let completed_before_release = !fallback_events.iter().any(|event| {
                    event.kind
                        == crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left)
                });
                let replay = self
                    .hits
                    .panes
                    .iter()
                    .any(|hit| hit.pane_id == pane_id && hit.inner_rect == inner_rect)
                    .then_some(fallback_events);
                if replay.is_none() {
                    self.url_click_consumes_until_up = completed_before_release;
                }
                let replay_action = |events: Option<Vec<crossterm::event::MouseEvent>>| {
                    events
                        .map(ClientShellAction::ReplayMouse)
                        .into_iter()
                        .collect()
                };
                return match result {
                    Ok(crate::api::schema::ResponseResult::PaneLinkActivated {
                        handled: true,
                        ..
                    }) => {
                        self.url_click_consumes_until_up = completed_before_release;
                        (false, Vec::new())
                    }
                    Ok(crate::api::schema::ResponseResult::PaneLinkActivated {
                        url: Some(url),
                        handled: false,
                    }) if crate::app::actions::safe_web_url(&url).is_some() => {
                        self.url_click_consumes_until_up = completed_before_release;
                        (false, vec![ClientShellAction::OpenSafeWebUrl(url)])
                    }
                    Ok(crate::api::schema::ResponseResult::PaneLinkActivated { .. }) => {
                        (false, replay_action(replay))
                    }
                    Ok(_) => {
                        self.endpoint_error =
                            Some("endpoint returned an unexpected link result".to_owned());
                        (true, replay_action(replay))
                    }
                    Err(error)
                        if matches!(
                            error.code.as_deref(),
                            Some("stale_content" | "stale_target" | "endpoint_cancelled")
                        ) =>
                    {
                        self.url_click_consumes_until_up = completed_before_release;
                        (false, Vec::new())
                    }
                    Err(_) => (true, replay_action(replay)),
                };
            }
        }
        (result.is_err(), Vec::new())
    }
}
